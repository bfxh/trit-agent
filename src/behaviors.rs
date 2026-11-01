//! 行为库：每个行为只投自己管得着的那几个通道，互相不直接调用。
//!
//! 「聪明」不体现在单个行为里，而体现在**仲裁**：一个行为可以投 `Zero`
//! 来表达「我反对但我说不出该往哪走」，权重够大就会把其他行为的 Pos 压成弃权。
//! 例如血量低又没交火时，`Regen` 用一条高权重弃权票把 `Patrol` 的前进压掉，
//! 角色就地站住回血 —— 这比在代码里写 `if low_hp { return }` 干净得多。

use crate::config::{Config, Mode, Tactics};
use crate::fsm::{Channel, Phase, Vote, WorldView};
use crate::trit::Tri;

pub struct Ctx {
    pub w: WorldView,
    pub phase: Phase,
    pub now_ms: u64,
    pub dt_ms: u64,
    /// 采集门控本拍给出的模式。
    ///
    /// 「该不该采」在 [`crate::gather`] 里判完才传进来，行为层只管
    /// 「手怎么动」—— 这样门控可以独立单测，不用构造一整个 Agent。
    pub gather: crate::gather::Mode,
}

pub trait Behavior {
    fn name(&self) -> &'static str;
    fn vote(&mut self, c: &Ctx) -> Vec<Vote>;
}

/// 交战：按节奏挥击、跳劈、左右换边。
pub struct Combat {
    next_attack: u64,
    attacks: u32,
    jump_until: u64,
    flip_at: u64,
    side: Tri,
    interval: u64,
    crit_every: u32,
    flip_ms: u64,
}

impl Combat {
    pub fn new(cfg: &Config) -> Combat {
        Combat {
            next_attack: 0,
            attacks: 0,
            jump_until: 0,
            flip_at: 0,
            side: Tri::Pos,
            interval: cfg.attack_interval_ms,
            crit_every: cfg.jump_crit_every,
            flip_ms: cfg.strafe_flip_ms,
        }
    }
}

impl Behavior for Combat {
    fn name(&self) -> &'static str {
        "combat"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Engage {
            return Vec::new();
        }
        let mut v = Vec::with_capacity(4);

        if c.now_ms >= self.flip_at {
            self.side = self.side.not();
            self.flip_at = c.now_ms + self.flip_ms;
        }
        v.push(Vote::new(Channel::Strafe, self.side, 2, "combat"));

        if c.now_ms >= self.next_attack {
            v.push(Vote::new(Channel::Attack, Tri::Pos, 2, "combat"));
            self.next_attack = c.now_ms + self.interval;
            self.attacks = self.attacks.wrapping_add(1);
            if self.crit_every > 0 && self.attacks % self.crit_every == 0 {
                // 跳劈：跳起来在下落时命中
                self.jump_until = c.now_ms + 140;
            }
        }
        if c.now_ms < self.jump_until {
            v.push(Vote::new(Channel::Jump, Tri::Pos, 2, "combat_crit"));
        }

        // 交战时不扫视，只做小幅盯人
        v.push(Vote::new(Channel::Yaw, self.side, 1, "combat_track"));
        v
    }
}

/// 巡逻：一路向前，定期跳。
///
/// 扫视不在这里做。原先 `Patrol` 和 `IdleScan` 各有一套独立的计时器和方向，
/// 两者在 Idle 阶段同时投票、方向相反，加权和正好抵消 —— 表现是「视角永远
/// 不动」。同一件事只能有一个来源，所以扫视统一交给 `IdleScan`。
pub struct Patrol {
    jump_at: u64,
    jump_ms: u64,
    /// 不前进时（hold）只跳不冲
    advance: bool,
}

impl Patrol {
    pub fn new(cfg: &Config) -> Patrol {
        Patrol {
            jump_at: 0,
            jump_ms: cfg.patrol_jump_ms,
            advance: cfg.mode != Mode::Hold,
        }
    }
}

impl Behavior for Patrol {
    fn name(&self) -> &'static str {
        "patrol"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Idle {
            return Vec::new();
        }
        let mut v = Vec::with_capacity(2);
        if self.advance {
            v.push(Vote::new(Channel::Forward, Tri::Pos, 3, "patrol"));
            if c.now_ms >= self.jump_at {
                v.push(Vote::new(Channel::Jump, Tri::Pos, 2, "patrol_step"));
                self.jump_at = c.now_ms + self.jump_ms;
            }
        }
        v
    }
}

/// 撤退：边退边看，偶尔跳一下脱离。
pub struct Flee {
    jump_at: u64,
    side: Tri,
    flip_at: u64,
}

impl Flee {
    pub fn new() -> Flee {
        Flee {
            jump_at: 0,
            side: Tri::Pos,
            flip_at: 0,
        }
    }
}

impl Behavior for Flee {
    fn name(&self) -> &'static str {
        "flee"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Retreat {
            return Vec::new();
        }
        let mut v = vec![
            Vote::new(Channel::Forward, Tri::Neg, 4, "flee"),
            Vote::new(Channel::Strafe, self.side, 1, "flee_juke"),
        ];
        if c.now_ms >= self.flip_at {
            self.side = self.side.not();
            self.flip_at = c.now_ms + 700;
        }
        if c.now_ms >= self.jump_at {
            v.push(Vote::new(Channel::Jump, Tri::Pos, 2, "flee_hop"));
            self.jump_at = c.now_ms + 520;
        }
        v
    }
}

/// 待命扫视：没有交战时的观察行为。
pub struct IdleScan {
    sweep_at: u64,
    side: Tri,
}

impl IdleScan {
    pub fn new() -> IdleScan {
        IdleScan {
            sweep_at: 0,
            side: Tri::Pos,
        }
    }
}

impl Behavior for IdleScan {
    fn name(&self) -> &'static str {
        "idle_scan"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Idle {
            return Vec::new();
        }
        // 采集时不许乱转：走向目标那一段如果目标正好在正前方，Forage 不投 Yaw，
        // 这时 IdleScan 的权重 3 就成了唯一一张票 —— 角色会一边走一边左右摆头，
        // 走过去的方向全歪了。
        if c.gather.active() {
            self.sweep_at = c.now_ms + 1600;
            return Vec::new();
        }
        if c.now_ms >= self.sweep_at {
            self.side = self.side.not();
            self.sweep_at = c.now_ms + 1600;
        }
        vec![Vote::new(Channel::Yaw, self.side, 3, "idle_scan")]
    }
}

/// 吃东西：血量低且周围安全时，周期性右键。
///
/// 借鉴 `hostile_humans/FoodItemGoal` 与 `TameItemGoal`：它们都要求
/// 36.0 = 6² 格内没有威胁才低头吃。原版的 eat goal 没有这个条件，
/// 导致怪一边挨打一边嚼东西。
pub struct Eat {
    next: u64,
}

impl Eat {
    pub fn new() -> Eat {
        Eat { next: 0 }
    }
}

impl Behavior for Eat {
    fn name(&self) -> &'static str {
        "eat"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        let hungry = matches!(c.w.hp, Some(h) if h < 0.55);
        if !hungry || c.w.contact {
            return Vec::new();
        }
        // 6 格内有东西就不吃（npc::EAT_SAFE）
        if matches!(c.w.dist, Some(d) if d <= crate::npc::EAT_SAFE) {
            return Vec::new();
        }
        // 爆炸物附近更不该停下来吃
        if c.w.hazard == Tri::Pos {
            return Vec::new();
        }
        if c.now_ms >= self.next {
            self.next = c.now_ms + 900;
            return vec![Vote::new(Channel::Use, Tri::Pos, 3, "eat")];
        }
        Vec::new()
    }
}

/// 交战距离控制：太近就退半步，太远就压上去，**刚好就站住**。
///
/// 这一条是整套移植里最关键的一处。原来的 `Combat` 在交战阶段完全不投
/// `Forward`，等于「一旦进入交战就放弃走位」，角色要么被怪推着走，要么
/// 一路顶着按 W。而所有 mob mod 的 Goal 都只有「靠近」和「开火」两档，
/// 缺的正是中间那一档 —— 于是它们贴脸对撞。
///
/// 三值化之后中间那档是 `Zero`，用高权重弃权票表达「位置正好，别推我」，
/// 它天然压掉巡逻和其他前进票，却不会像 `Neg` 那样把人推回去。
pub struct Ranging;

impl Behavior for Ranging {
    fn name(&self) -> &'static str {
        "ranging"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Engage {
            return Vec::new();
        }
        let band = crate::npc::engage_band(c.w.dist);
        match band {
            // 太远：追
            Tri::Pos => vec![Vote::new(Channel::Forward, Tri::Pos, 4, "range_chase")],
            // 刚好：站住，别再往前顶
            Tri::Zero => vec![Vote::abstain(Channel::Forward, 5, "range_hold")],
            // 太近：退半步，同时侧滑免得一直被贴
            Tri::Neg => vec![
                Vote::new(Channel::Forward, Tri::Neg, 3, "range_back"),
                Vote::new(Channel::Strafe, Tri::Pos, 2, "range_slide"),
            ],
        }
    }
}

/// 爆炸物规避：苦力怕 10 格、TNT 3 格。
///
/// 直接用 `hostile_humans/AvoidCreeperGoal`(10.0) 和 `AvoidTNTGoal`(3.0)
/// 的阈值。原版 mob 用 `WalkTargetGoal` 找路逃，这里只能后退 + 跳，
/// 因为外部只能按键 —— 但至少不会站着被炸。
pub struct Hazard {
    next_hop: u64,
}

impl Hazard {
    pub fn new() -> Hazard {
        Hazard { next_hop: 0 }
    }
}

impl Behavior for Hazard {
    fn name(&self) -> &'static str {
        "hazard"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase == Phase::Off || c.phase == Phase::Recover {
            return Vec::new();
        }
        if c.w.hazard != Tri::Pos {
            return Vec::new();
        }
        // 权重 6：要能压过 range_chase(4) 和 patrol(3)
        let mut v = vec![
            Vote::new(Channel::Forward, Tri::Neg, 6, "hazard_back"),
            Vote::new(Channel::Strafe, Tri::Pos, 2, "hazard_side"),
        ];
        if c.now_ms >= self.next_hop {
            v.push(Vote::new(Channel::Jump, Tri::Pos, 3, "hazard_hop"));
            self.next_hop = c.now_ms + 400;
        }
        v
    }
}

/// 举盾：贴脸且没在出手时挡一下。
///
/// `enhancedai/ShieldingGoal` 里写死 30 tick = 1.5 秒。
/// 注意右键在 MC 里同时是「吃」和「挡」，谁生效取决于手上拿的是什么，
/// 所以这里只负责在**近战窗口**里按下去，进食的低血逻辑权重更低。
pub struct Guard {
    until: u64,
    next: u64,
}

impl Guard {
    pub fn new() -> Guard {
        Guard { until: 0, next: 0 }
    }
}

impl Behavior for Guard {
    fn name(&self) -> &'static str {
        "guard"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Engage {
            return Vec::new();
        }
        // 只在真正近身时举盾，中距离举盾是浪费
        let close = matches!(c.w.dist, Some(d) if d <= crate::npc::MELEE_RANGE + 1.0);
        if !close {
            return Vec::new();
        }
        // 只有掉过血才挡。满血近战时按住右键是危险的：手上拿食物的话
        // 右键会触发进食动画（1.6 秒不能攻击），比不挡更糟。
        let hurt = matches!(c.w.hp, Some(h) if h < 0.6);
        if !hurt {
            return Vec::new();
        }
        let hold_ms = crate::npc::SHIELD_TICKS * 50; // 30 tick @ 20TPS
        if c.now_ms >= self.next {
            self.until = c.now_ms + hold_ms;
            self.next = c.now_ms + hold_ms + 700;
        }
        if c.now_ms < self.until {
            // 权重 4 > eat 的 3，近身时格挡优先于进食
            vec![Vote::new(Channel::Use, Tri::Pos, 4, "guard_block")]
        } else {
            Vec::new()
        }
    }
}

/// 卡住了就拆墙：往下看 + 左键。
///
/// `improvedmobs/BlockBreakGoal`（常量 0.5 / 1.1）和 `enhancedai/DiggingGoal`
/// （4.0, 1.5, 2.25）做的是同一件事：追不上就挖穿它。外部驱动能做的
/// 最小版本是「低头砍几刀」—— 砍的是方块还是怪的脚，取决于视线。
pub struct Breach {
    until: u64,
    next: u64,
}

impl Breach {
    pub fn new() -> Breach {
        Breach { until: 0, next: 0 }
    }
}

impl Behavior for Breach {
    fn name(&self) -> &'static str {
        "breach"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if !c.w.stuck {
            self.until = 0;
            return Vec::new();
        }
        // 采集期间「卡住」是正常的：挖矿本来就不移动，`stuck` 判据一定成立。
        // 这时候再让 Breach 插手，它会一边投票往下看一边按住左键，把采集的
        // 瞄准闭环和挖掘进度全打乱 —— 表现出来是「AI 对着矿旁边那面墙猛挖」。
        if c.gather.active() {
            self.until = 0;
            return Vec::new();
        }
        if c.phase != Phase::Engage && c.phase != Phase::Idle {
            return Vec::new();
        }
        if c.now_ms >= self.next {
            // 挖一小段就停，别把整面墙拆了
            self.until = c.now_ms + 900;
            self.next = c.now_ms + 2600;
        }
        if c.now_ms >= self.until {
            return Vec::new();
        }
        vec![
            Vote::new(Channel::Pitch, Tri::Pos, 3, "breach_look"),
            Vote::new(Channel::Attack, Tri::Pos, 3, "breach_hit"),
        ]
    }
}

/// 巡逻半径闸门：走太远就停下扫视。
///
/// `humancompanions/PatrolGoal` 和 `MoveBackToPatrolGoal` 是一对 —— 后者负责
/// 「离岗了就回去」。回去需要知道朝向，而外部驱动读不到 yaw，所以这里只能做
/// 边界那一半：**到了半径就不许再走**，剩下的交给玩家。宁可站住，也不要
/// 一路游走到几千格外。
pub struct Anchor {
    center: Option<(f32, f32)>,
    radius: f32,
    sweep_at: u64,
    side: Tri,
}

impl Anchor {
    pub fn new(radius: f32) -> Anchor {
        Anchor {
            center: None,
            radius,
            sweep_at: 0,
            side: Tri::Pos,
        }
    }
}

impl Behavior for Anchor {
    fn name(&self) -> &'static str {
        "anchor"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Idle {
            return Vec::new();
        }
        let Some((x, _, z)) = c.w.pos else {
            return Vec::new();
        };
        let center = *self.center.get_or_insert((x, z));
        let d = ((x - center.0).powi(2) + (z - center.1).powi(2)).sqrt();
        if d <= self.radius {
            return Vec::new();
        }
        if c.now_ms >= self.sweep_at {
            self.side = self.side.not();
            self.sweep_at = c.now_ms + 1400;
        }
        // 权重 6 压住 patrol 的前进票：到边界了就停，只转头
        vec![
            Vote::abstain(Channel::Forward, 6, "anchor_edge"),
            Vote::new(Channel::Yaw, self.side, 3, "anchor_scan"),
        ]
    }
}

/// 回血：低血、无敌情时，用高权重弃权票把移动压掉。
pub struct Regen;

impl Behavior for Regen {
    fn name(&self) -> &'static str {
        "regen"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.w.contact || c.phase != Phase::Idle {
            return Vec::new();
        }
        let low = matches!(c.w.hp, Some(h) if h < 0.6);
        if !low {
            return Vec::new();
        }
        vec![
            Vote::abstain(Channel::Forward, 6, "regen_stand"),
            Vote::abstain(Channel::Strafe, 4, "regen_stand"),
        ]
    }
}


/// 采集：走过去、低头、按住左键，中间抬头看周围。
///
/// 这个类本身**不做任何安全判断**。`c.gather` 是 [`crate::gather::Gather`]
/// 判完之后传进来的结果，这里只负责把那个结论翻译成按键。
/// 分开的好处是门控能单测（不用造整个 Agent），行为层也保持"只投票"的纯洁性。
///
/// 两条容易写错的：
/// 1. 砍树要 [`Vote::hold`]（按住），不是 `Vote::new(Pos)`（点一下）。
///    点一下的脉冲只有 70ms，方块挖掘进度会不断归零，永远砍不断。
/// 2. 扫视时**一条通道都不投**。裁决器里只有"本拍没人投票"才会松手，
///    投弃权票压不掉上一拍的 `Pos`（迟滞会把它留在那儿）。
pub struct Forage {
    yaw_at: u64,
    yaw_side: Tri,
    /// 本次俯仰修正推到什么时候
    pitch_until: u64,
    /// 下次允许开始新的修正（等数据包把新角度报回来的时间）
    pitch_next: u64,
    /// 视角转速（度/秒），用来算"推多久刚好转到位"
    look_rate: f32,
}

impl Forage {
    pub fn new(look_rate: f32) -> Forage {
        Forage {
            yaw_at: 0,
            yaw_side: Tri::Pos,
            pitch_until: 0,
            pitch_next: 0,
            look_rate: look_rate.max(1.0),
        }
    }

    /// 俯仰修正：推一下 → 停手等感知 → 再推。
    ///
    /// 为什么不每拍都投：感知是等间隔的，而视线默认转 55°/s。一直投 `Pos`
    /// 的话，在"看不见"的那段区间里能转过头一大截 —— 1 秒档时是 55°，
    /// 直接从树干冲到脚面，然后下一拍又往回转，振荡，永远对不准。
    ///
    /// 所以按「角度差 ÷ 转速」算出这次该推多久，推完就停手，等新读数。
    ///
    /// **等多久不写死**：写 900ms 的话装了快通道也白装（它 100ms 就有一帧），
    /// 写 100ms 的话只有慢通道时一路转过头。所以用实测的帧间隔 `gap` 推：
    /// 留 40% 余量，免得在"新帧还没到"的时候又推一次，两次推行叠成一个。
    fn pitch_vote(&mut self, cur: Option<f32>, want: f32, now: u64, gap: Option<f32>) -> Option<Tri> {
        let dir = crate::gather::aim(cur, want)?;
        if now < self.pitch_until {
            return Some(dir); // 还在推，继续
        }
        if now < self.pitch_next {
            return None; // 推完了，等感知刷新，这期间一个字都不投
        }
        let deg = (want - cur.unwrap_or(want)).abs();
        let burst = ((deg / self.look_rate) * 1000.0).clamp(80.0, 400.0) as u64;
        self.pitch_until = now + burst;
        self.pitch_next = now + burst + Self::wait_ms(gap);
        Some(dir)
    }

    /// 推完之后要等多久才开始下一轮。
    fn wait_ms(gap: Option<f32>) -> u64 {
        let g = gap.unwrap_or(1_000.0).clamp(60.0, 1_400.0);
        ((g * 1.4) as u64 + 30).clamp(90, 1_700)
    }
}

impl Default for Forage {
    fn default() -> Self {
        Forage::new(55.0)
    }
}

impl Behavior for Forage {
    fn name(&self) -> &'static str {
        "forage"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        use crate::gather::{Mode, Target};
        // 只在 Idle 阶段采集。交战/撤退时 phase 已经不是 Idle ——
        // 不需要在这里再写一遍"有威胁就停"，那是重复且容易漏的门控。
        if c.phase != Phase::Idle {
            return Vec::new();
        }
        // 采什么由 mode 反推，不再单独存一份：`Mine` 就是挖矿，其余按砍树处理。
        // 距离和方位都从 `c.w` 现取 —— 门控那一拍已经算过，这里只是同一份
        // 世界观的另一种读法，避免两处各算一套然后对不上。
        let target = match c.gather {
            Mode::Mine => Target::Ore,
            _ => Target::Wood,
        };
        let dir = target.dir(&c.w).unwrap_or(0);
        let dist = target.dist(&c.w).unwrap_or(3.0);
        let want_pitch = match target {
            Target::Ore => crate::gather::aim_pitch(dist),
            Target::Wood => crate::gather::PITCH_CHOP,
        };
        match c.gather {
            // 冷却中让位：这时候应该走得越远越好，交给巡逻/撤退
            Mode::Off | Mode::Cooldown => Vec::new(),

            // 想采但安全窗口还没攒够：**站住等**，别顺手巡逻走开。
            // 一走开目标就出视野了，等于每 2 秒重新开始。
            Mode::Wait => {
                vec![Vote::abstain(Channel::Forward, 5, "forage_hold")]
            }

            // 插播扫视：停手、抬头、左右看。
            Mode::Scan => {
                if c.now_ms >= self.yaw_at {
                    self.yaw_side = self.yaw_side.not();
                    self.yaw_at = c.now_ms + 420;
                }
                // 只水平扫，**不动视线俯仰**。
                //
                // 原来这里会把视线抬回 -8°，本意是"抬头看看远处有没有怪"。
                // 但威胁不是靠看的 —— 数据包一直在报 16 格内的敌人数和距离档。
                // 抬视线的唯一实际效果是把正在挖的方块丢掉，然后花大半秒重新
                // 压回去：扫视 350ms + 重新对准 600ms，每 1.6 秒来一次，
                // 等于一半时间在跟自己的视线较劲。
                vec![Vote::new(Channel::Yaw, self.yaw_side, 4, "forage_scan")]
            }

            // 走过去。朝向只给方向，转多少由 look_rate 决定（每秒重测，闭环修正）。
            Mode::Approach => {
                // 权重 4 > patrol 的 3：采集时走采集的路，不是巡逻的路
                let mut v = vec![Vote::new(Channel::Forward, Tri::Pos, 4, "forage_walk")];
                if dir != 0 {
                    let yaw = if dir < 0 { Tri::Neg } else { Tri::Pos };
                    v.push(Vote::new(Channel::Yaw, yaw, 4, "forage_turn"));
                }
                // 平视：一边走一边低头会撞进水里 / 掉坑里
                if let Some(t) = self.pitch_vote(
                    c.w.pitch,
                    crate::gather::PITCH_WALK,
                    c.now_ms,
                    c.w.sense_gap_ms,
                ) {
                    v.push(Vote::new(Channel::Pitch, t, 1, "forage_level"));
                }
                // 前方是一格高的台阶：跳过去。原版跳跃能上 1.25 格，
                // 比"绕路"现实得多 —— 绕路需要路径规划，那是另一个量级的事。
                //
                // 只在 `step` 明确为真时跳。`wall` 为真而 `step` 未知时也跳：
                // 撞上去之前跳一下没有代价，但站着不动会被看成卡死。
                let ahead = c.w.wall.map(|w| crate::perceive::wall_at(w, 0)) == Some(true);
                if ahead && c.w.step != Some(false) {
                    v.push(Vote::new(Channel::Jump, Tri::Pos, 3, "forage_step_up"));
                }
                v
            }

            // 砍树 / 挖矿。按住左键不放，视线压到目标高度。
            Mode::Chop | Mode::Mine => {
                let tag = if c.gather == Mode::Mine { "mine" } else { "chop" };
                let mut v = vec![Vote::hold(Channel::Attack, 5, tag)];
                // 视线闭环压到目标高度。这一步不做的话，`Pos` 会一路转到脚面，
                // 准星落在地面上，左键按着也只是在刨土。
                //
                // 砍树是固定 22°；挖矿按距离算 —— 脚边的矿和隔着三格的矿
                // 需要的角度差一倍，写死一个值总有一边瞄不到。
                if let Some(t) =
                    self.pitch_vote(c.w.pitch, want_pitch, c.now_ms, c.w.sense_gap_ms)
                {
                    v.push(Vote::new(Channel::Pitch, t, 3, "forage_aim_down"));
                }
                // 目标只有 1 格宽，走位会偏，时不时重新对一次
                if dir != 0 {
                    let yaw = if dir < 0 { Tri::Neg } else { Tri::Pos };
                    v.push(Vote::new(Channel::Yaw, yaw, 3, "forage_aim"));
                }
                v
            }
        }
    }
}

// 这些行为都是无参构造，补一个 Default 让 clippy 闭嘴，也方便测试里 `Default::default()`。
impl Default for Flee {
    fn default() -> Self {
        Flee::new()
    }
}
impl Default for IdleScan {
    fn default() -> Self {
        IdleScan::new()
    }
}
impl Default for Eat {
    fn default() -> Self {
        Eat::new()
    }
}
impl Default for Hazard {
    fn default() -> Self {
        Hazard::new()
    }
}
impl Default for Guard {
    fn default() -> Self {
        Guard::new()
    }
}
impl Default for Breach {
    fn default() -> Self {
        Breach::new()
    }
}

/// 把所有行为装在一起，按配置启用。
pub struct Behaviors {
    pub combat: Combat,
    pub patrol: Patrol,
    pub flee: Flee,
    pub scan: IdleScan,
    pub eat: Eat,
    pub regen: Regen,
    /// 以下来自 mob mod 的 Goal 移植，见 [`crate::npc`]
    pub ranging: Ranging,
    pub hazard: Hazard,
    pub guard: Guard,
    pub breach: Breach,
    pub anchor: Anchor,
    /// 采集（砍树）。只有在 `c.gather` 说能动手时才投票。
    pub forage: Forage,
    /// 战术开关：关掉的行为完全不投票，等于回到没有那一档的状态
    tac: Tactics,
}

impl Behaviors {
    pub fn new(cfg: &Config) -> Behaviors {
        Behaviors {
            combat: Combat::new(cfg),
            patrol: Patrol::new(cfg),
            flee: Flee::new(),
            scan: IdleScan::new(),
            eat: Eat::new(),
            regen: Regen,
            ranging: Ranging,
            hazard: Hazard::new(),
            guard: Guard::new(),
            breach: Breach::new(),
            anchor: Anchor::new(crate::npc::PATROL_RADIUS),
            forage: Forage::new(cfg.look_rate),
            tac: cfg.tactics,
        }
    }

    pub fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        let mut out = Vec::with_capacity(16);
        // 危险规避最先入列：它的票权重最高，顺序只影响日志可读性
        if self.tac.hazard {
            out.extend(self.hazard.vote(c));
        }
        out.extend(self.combat.vote(c));
        if self.tac.range {
            out.extend(self.ranging.vote(c));
        }
        if self.tac.guard {
            out.extend(self.guard.vote(c));
        }
        if self.tac.breach {
            out.extend(self.breach.vote(c));
        }
        out.extend(self.flee.vote(c));
        out.extend(self.regen.vote(c));
        out.extend(self.eat.vote(c));
        out.extend(self.anchor.vote(c));
        out.extend(self.forage.vote(c));
        out.extend(self.patrol.vote(c));
        out.extend(self.scan.vote(c));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsm::Machine;

    fn ctx(phase: Phase, now: u64, hp: f32, contact: bool) -> Ctx {
        Ctx {
            w: WorldView {
                hp: Some(hp),
                contact,
                threat: if contact { Tri::Pos } else { Tri::Neg },
                ..Default::default()
            },
            phase,
            now_ms: now,
            dt_ms: 5,
            gather: crate::gather::Mode::Off,
        }
    }

    #[test]
    fn forage_aims_pitch_in_bursts_not_continuously() {
        // 数据包 1 秒报一次角度，视线 55°/s。如果每拍都投 Pos，1 秒能转 55°，
        // 从树干一路冲到脚面。所以要「推一下 → 等感知 → 再推」。
        let mut f = Forage::new(55.0);
        let mk = |now: u64, pitch: f32| Ctx {
            w: WorldView {
                hp: Some(1.0),
                threat: Tri::Neg,
                wood: Some(3.0),
                wood_dir: Some(0),
                pitch: Some(pitch),
                food: Some(18),
                night: Some(false),
                ..Default::default()
            },
            phase: Phase::Idle,
            now_ms: now,
            dt_ms: 5,
            gather: crate::gather::Mode::Chop,
        };

        let pitch_of = |f: &mut Forage, c: &Ctx| {
            f.vote(c)
                .into_iter()
                .find(|v| v.channel == Channel::Pitch)
                .map(|v| v.dir)
        };

        // 0° → 目标 22°：开推
        assert_eq!(pitch_of(&mut f, &mk(0, 0.0)), Some(Tri::Pos));
        // 推的窗口内还在推
        assert_eq!(pitch_of(&mut f, &mk(200, 0.0)), Some(Tri::Pos));
        // 推完了（22°/55° ≈ 400ms）：停手等感知，这一拍不该有任何俯仰票
        assert_eq!(pitch_of(&mut f, &mk(500, 0.0)), None);
        // 新读数回来了，已经对准 → 还是不动
        assert_eq!(pitch_of(&mut f, &mk(1_500, 21.0)), None);
        // 转过头了（44°）→ 往回修
        assert_eq!(pitch_of(&mut f, &mk(2_600, 44.0)), Some(Tri::Neg));
    }

    #[test]
    fn forage_never_holds_attack_while_scanning() {
        // 扫视时一条攻击票都不能有：裁决器里"本拍没人投票"才松手，
        // 投弃权票压不掉上一拍的 Pos。
        let mut f = Forage::new(55.0);
        let c = Ctx {
            w: WorldView {
                hp: Some(1.0),
                threat: Tri::Neg,
                wood: Some(3.0),
                wood_dir: Some(0),
                pitch: Some(22.0),
                food: Some(18),
                night: Some(false),
                ..Default::default()
            },
            phase: Phase::Idle,
            now_ms: 5_000,
            dt_ms: 5,
            gather: crate::gather::Mode::Scan,
        };
        let v = f.vote(&c);
        assert!(v.iter().all(|x| x.channel != Channel::Attack), "{v:?}");
        assert!(v.iter().all(|x| x.channel != Channel::Forward), "{v:?}");
    }

    #[test]
    fn forage_is_silent_when_the_gate_says_no() {
        let mut f = Forage::new(55.0);
        for mode in [
            crate::gather::Mode::Off,
            crate::gather::Mode::Cooldown,
        ] {
            let c = Ctx {
                w: WorldView {
                    hp: Some(1.0),
                    wood: Some(3.0),
                    ..Default::default()
                },
                phase: Phase::Idle,
                now_ms: 0,
                dt_ms: 5,
                gather: mode,
            };
            assert!(f.vote(&c).is_empty(), "{mode:?} 不该投票");
        }
    }

    #[test]
    fn forage_waits_instead_of_wandering_off() {
        // Wait = 想采但还没攒够安全窗口。这时候必须站住，
        // 不然一顺手巡逻走开，木头就出视野了，每 2 秒从头再来。
        let mut f = Forage::new(55.0);
        let c = Ctx {
            w: WorldView {
                hp: Some(1.0),
                wood: Some(3.0),
                ..Default::default()
            },
            phase: Phase::Idle,
            now_ms: 0,
            dt_ms: 5,
            gather: crate::gather::Mode::Wait,
        };
        let v = f.vote(&c);
        assert!(v.iter().any(|x| x.channel == Channel::Forward && x.dir == Tri::Zero));
    }

    #[test]
    fn regen_suppresses_patrol_via_unknown_votes() {
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        // 低血、无交火：Patrol 想前进(3)，Regen 弃权(6) 应把它压掉
        let c = ctx(Phase::Idle, 1000, 0.3, false);
        let votes = b.vote(&c);
        let d = m.step(&c.w, &votes, &all);
        assert_eq!(d.phase, Phase::Idle);
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Zero);
    }

    #[test]
    fn healthy_patrol_advances() {
        let mut cfg = Config::default();
        cfg.mode = Mode::Patrol;
        let mut b = Behaviors::new(&cfg);
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        let c = ctx(Phase::Idle, 1000, 1.0, false);
        let votes = b.vote(&c);
        let d = m.step(&c.w, &votes, &all);
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Pos);
    }

    #[test]
    fn combat_attacks_on_rhythm() {
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        let c = ctx(Phase::Engage, 0, 1.0, true);
        let votes = b.vote(&c);
        let d = m.step(&c.w, &votes, &all);
        assert_eq!(d.dirs[Channel::Attack.index()], Tri::Pos);
    }

    /// 交战态 + 指定距离/卡住状态的世界观。
    fn engaged(dist: Option<f32>, stuck: bool) -> Ctx {
        Ctx {
            w: WorldView {
                hp: Some(1.0),
                contact: true,
                threat: Tri::Pos,
                dist,
                stuck,
                ..Default::default()
            },
            phase: Phase::Engage,
            now_ms: 1000,
            gather: crate::gather::Mode::Off,
            dt_ms: 5,
        }
    }

    fn dirs_of(b: &mut Behaviors, c: &Ctx) -> [Tri; 7] {
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        let votes = b.vote(c);
        m.step(&c.w, &votes, &all).dirs
    }

    #[test]
    fn range_holds_at_mid_distance() {
        // 5 格：在原版 Goal 眼里只有「再近点」和「开火」两档，会一直往前挤。
        // 移植后这一段是 Zero —— 站住，别再顶上去。
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let d = dirs_of(&mut b, &engaged(Some(5.0), false));
        assert_eq!(d[Channel::Forward.index()], Tri::Zero);
    }

    #[test]
    fn range_closes_when_far() {
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let d = dirs_of(&mut b, &engaged(Some(20.0), false));
        assert_eq!(d[Channel::Forward.index()], Tri::Pos);
    }

    #[test]
    fn range_backs_off_when_hugging() {
        // 贴脸 1.5 格：小退一步，但仍留在近战距离内
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let d = dirs_of(&mut b, &engaged(Some(1.5), false));
        assert_eq!(d[Channel::Forward.index()], Tri::Neg);
    }

    #[test]
    fn hazard_overrides_chase() {
        // 敌人在 20 格（想追），但苦力怕贴脸 —— 躲优先于追
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let mut c = engaged(Some(20.0), false);
        c.w.hazard = Tri::Pos;
        let d = dirs_of(&mut b, &c);
        assert_eq!(d[Channel::Forward.index()], Tri::Neg, "必须先退");
    }

    #[test]
    fn stuck_breaks_out() {
        // 卡住 1.2 秒 → 低头砍墙（improvedmobs/BlockBreakGoal 的等价物）
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let d = dirs_of(&mut b, &engaged(Some(20.0), true));
        assert_eq!(d[Channel::Pitch.index()], Tri::Pos);
        assert_eq!(d[Channel::Attack.index()], Tri::Pos);
    }

    #[test]
    fn not_stuck_does_not_dig() {
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let d = dirs_of(&mut b, &engaged(Some(20.0), false));
        assert_ne!(d[Channel::Pitch.index()], Tri::Pos);
    }

    #[test]
    fn unknown_distance_does_not_charge() {
        // 没距离信息：不许猜成前进。宁可站住等人，也不盲目冲。
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let d = dirs_of(&mut b, &engaged(None, false));
        assert_ne!(d[Channel::Forward.index()], Tri::Pos);
    }

    #[test]
    fn guard_raises_shield_only_up_close() {
        let cfg = Config::default();
        // 受伤 + 贴脸 → 举盾
        let mut hurt = engaged(Some(3.0), false);
        hurt.w.hp = Some(0.4);
        let mut b = Behaviors::new(&cfg);
        let near = dirs_of(&mut b, &hurt);
        assert_eq!(near[Channel::Use.index()], Tri::Pos, "受伤且贴脸应举盾");

        // 受伤但太远 → 不该浪费盾
        let mut far = engaged(Some(20.0), false);
        far.w.hp = Some(0.4);
        let mut b2 = Behaviors::new(&cfg);
        assert_ne!(
            dirs_of(&mut b2, &far)[Channel::Use.index()],
            Tri::Pos,
            "远处不该举盾"
        );

        // 满血贴脸 → 不举盾。手上是食物时按右键会触发进食动画，
        // 1.6 秒打不出伤害，比不挡更糟。
        let mut full = engaged(Some(3.0), false);
        full.w.hp = Some(1.0);
        let mut b3 = Behaviors::new(&cfg);
        assert_ne!(
            dirs_of(&mut b3, &full)[Channel::Use.index()],
            Tri::Pos,
            "满血不该按右键"
        );
    }

    #[test]
    fn anchor_stops_patrol_at_radius() {
        // 离起点很远了：巡逻不许继续前进
        let mut cfg = Config::default();
        cfg.mode = Mode::Patrol;
        let mut b = Behaviors::new(&cfg);

        // 第一拍在起点：锚点定在这里，正常巡逻
        let mut start = ctx(Phase::Idle, 1000, 1.0, false);
        start.w.pos = Some((0.0, 64.0, 0.0));
        let d0 = dirs_of(&mut b, &start);
        assert_eq!(d0[Channel::Forward.index()], Tri::Pos, "起点应该正常前进");

        // 走远 100 格：该停住
        let mut far = ctx(Phase::Idle, 2000, 1.0, false);
        far.w.pos = Some((100.0, 64.0, 0.0));
        let d = dirs_of(&mut b, &far);
        assert_eq!(d[Channel::Forward.index()], Tri::Zero);
    }

    #[test]
    fn idle_scan_flips_and_crosses_hysteresis() {
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        let mut out = Vec::new();
        for t in [0u64, 100, 1500, 1600, 1700, 3200] {
            let c = Ctx {
                w: WorldView {
                    hp: Some(1.0),
                    threat: Tri::Neg,
                    ..Default::default()
                },
                phase: Phase::Idle,
                now_ms: t,
                dt_ms: 5,
                gather: crate::gather::Mode::Off,
            };
            let v = b.vote(&c);
            let d = m.step(&c.w, &v, &all);
            out.push((t, d.dirs[Channel::Yaw.index()]));
        }
        assert_eq!(out[0].1, Tri::Neg, "t=0 应向左");
        assert_eq!(out[3].1, Tri::Pos, "t=1600 应翻向右");
        assert_eq!(out[5].1, Tri::Neg, "t=3200 应翻回左");
    }
}
