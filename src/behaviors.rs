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
        }
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
