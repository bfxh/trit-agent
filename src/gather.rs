//! 采集（砍树 / 挖矿）：真正难的从来不是"怎么砍"，而是"什么时候不砍"。
//!
//! 一个只会砍树的 bot 在生存里是负资产：它会在你被三只僵尸追着跑的时候
//! 突然转身去砍旁边那棵树，然后你死了它还站着。所以这里的门控不是
//! `if hp > 0.5`，而是一个带**非对称进入/退出**的三值判据：
//!
//! - **进入**要连续 [`GatherCfg::enter_ms`] 毫秒「明确安全」，一次不确定就重头计；
//! - **退出**是当拍的，慢一点就来不及了；
//! - 因为威胁退出会吃一次 [`GatherCfg::cooldown_ms`] 的惩罚冷却。
//!
//! 最后这条是整套东西里最容易被忽略、但最影响观感的：没有惩罚冷却，
//! 怪物稍微一走远它就回去砍，砍两下怪物又回来 —— 表现为原地抽搐。
//! 真人不会这么干，真人会「这地方不安全了，先走」。
//!
//! 判据全部是三值的，`Neg` 明确不许、`Zero` 不确定（也不许，但不算"危险"）、
//! `Pos` 明确可以。合成规则：任一 `Neg` 压过全部，任一 `Zero` 降级为不确定。
//!
//! ## 挖矿比砍树多出来的三条
//!
//! 砍树站着不动就行，挖矿是**在改变自己脚下的地形**，所以多了三条判据，
//! 每一条都是拿一次死亡换来的：
//!
//! 1. **脚下要有支撑**（[`Why::NoFloor`]）。挖矿时人会不自觉地往前挪，
//!    前方是刚挖开的坑，脚下那块一旦挖掉就掉下去 —— 而坑底可能是岩浆。
//! 2. **目标不能是岩浆**（[`Why::Lava`]）。这条看起来废话，但矿石 tag 和
//!    岩浆 tag 是两套判据，`blk_front` 只报一个类别，必须先看危险再看收益。
//! 3. **前方被挡就别硬走**（[`Why::Blocked`]）。地上砍树能绕，地下矿道不能：
//!    对着墙走十秒和放弃换目标是同一件事，但前者看起来像卡死。

use crate::config::GatherCfg;
use crate::fsm::WorldView;
use crate::perceive::Block;
use crate::trit::Tri;

/// 采什么。两种目标的门控和瞄准都不同。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Target {
    #[default]
    Wood,
    Ore,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Wood => "木头",
            Target::Ore => "矿石",
        }
    }
    pub fn parse(s: &str) -> Option<Target> {
        match s {
            "wood" | "tree" | "木" | "树" => Some(Target::Wood),
            "ore" | "mine" | "矿" => Some(Target::Ore),
            _ => None,
        }
    }
    /// 这个目标现在离得多远。`None` = 视野里没有。
    pub fn dist(self, w: &WorldView) -> Option<f32> {
        match self {
            Target::Wood => w.wood,
            Target::Ore => w.ore,
        }
    }
    pub fn dir(self, w: &WorldView) -> Option<i32> {
        match self {
            Target::Wood => w.wood_dir,
            Target::Ore => w.ore_dir,
        }
    }
}

/// 采集过程中的内部模式。对外只用于观察，不参与通道裁决。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// 没在采集（配置关着，或配额已满在休息）
    Off,
    /// 想采，但安全窗口还没攒够 —— 站着等，不动手
    Wait,
    /// 朝目标走过去
    Approach,
    /// 砍树
    Chop,
    /// 挖矿
    Mine,
    /// 采集中的插播：停手抬头看周围
    Scan,
    /// 被打断了，歇一会儿
    Cooldown,
}

impl Mode {
    /// 给人看的中文名（`!gather`、演示表格）。
    ///
    /// 日志里的 `Decision::encode` 走的是 `Phase::name`，和这里无关 ——
    /// 所以改中文不会破坏 `trit-agent-run.log` 的机器可读格式。
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "停工",
            Mode::Wait => "等安全",
            Mode::Approach => "接近",
            Mode::Chop => "砍伐",
            Mode::Mine => "挖掘",
            Mode::Scan => "扫视",
            Mode::Cooldown => "冷却",
        }
    }
    /// 这个模式下 AI 是否该真的动手（走 / 砍 / 挖）
    pub fn active(self) -> bool {
        matches!(self, Mode::Approach | Mode::Chop | Mode::Mine)
    }
    /// 这个模式下需要按住左键
    pub fn holding(self) -> bool {
        matches!(self, Mode::Chop | Mode::Mine)
    }
}

/// 「为什么要停工」的原因。给 `!gather` 和演示表格用 —— 中文，因为
/// 这一层是给人读的：看到「血太少」比看到 `low_hp` 快得多。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    Disabled,
    QuotaDone,
    Cooldown,
    /// 明确有威胁
    Threat,
    /// 威胁状态未知 —— 不猜，不干活
    ThreatUnknown,
    LowHp,
    HpMarginal,
    HpUnknown,
    Hungry,
    FoodUnknown,
    Hazard,
    EnemyNear,
    Night,
    NightUnknown,
    NoWood,
    NoOre,
    TooFar,
    /// 砍/挖太久还没断，多半判断错了（树在墙后 / 在坡上 / 工具不对）
    LostTree,
    /// 脚下没东西 —— 挖下去就是掉下去
    NoFloor,
    /// 目标或脚下是岩浆
    Lava,
    /// 前方被固体挡住，走不过去
    Blocked,
    /// 方块信息还没到（快通道没开）
    NoBlockData,
    /// 明确安全，但「连续安全」的窗口还没攒够 —— 站着数秒
    Warmup,
    Ok,
}

impl Why {
    pub fn as_str(self) -> &'static str {
        match self {
            Why::Disabled => "已关闭",
            Why::QuotaDone => "配额满",
            Why::Cooldown => "冷却中",
            Why::Threat => "有敌人",
            Why::ThreatUnknown => "威胁未知",
            Why::LowHp => "血太少",
            Why::HpMarginal => "血量临界",
            Why::HpUnknown => "血量未知",
            Why::Hungry => "饿了",
            Why::FoodUnknown => "饱食未知",
            Why::Hazard => "有爆炸源",
            Why::EnemyNear => "敌人太近",
            Why::Night => "天黑了",
            Why::NightUnknown => "时间未知",
            Why::NoWood => "没树",
            Why::NoOre => "没矿",
            Why::TooFar => "太远",
            Why::LostTree => "目标丢了",
            Why::NoFloor => "脚下悬空",
            Why::Lava => "有岩浆",
            Why::Blocked => "被挡住",
            Why::NoBlockData => "方块未知",
            Why::Warmup => "攒安全",
            Why::Ok => "可以干",
        }
    }
    /// 这一条属于「危险」，该吃惩罚冷却；「没木头」「配额满了」这种不算。
    pub fn is_unsafe(self) -> bool {
        matches!(
            self,
            Why::Threat
                | Why::ThreatUnknown
                | Why::Hazard
                | Why::EnemyNear
                | Why::LowHp
                | Why::LostTree
                | Why::Lava
                | Why::NoFloor
        )
    }
}

/// 范围门：这些格数以内出现敌对生物就不砍。
///
/// 取 16 是数据包 `d=` 距离档的上界档。玩家的反应时间 + 收手 + 转身
/// 大约要 1.5 秒，16 格大约正好是僵尸走过来的一半路。
pub const ENEMY_CLEAR: f32 = 16.0;

/// 数据包报的原木距离档 -> 格数。
///
/// 档位是 `[12, 6, 3]`（见 `make_trit_bridge.py`，在 bfxh/trit-mc 仓库里）。`0` = 8 个方向
/// 都没命中 = 视野内没有木头，映射成 `None`。
///
/// 注意这是**下界**：报 3 意味着"3 格内有"，实际可能是 1.5 格。用来决定
/// "继续走"还是"开砍"足够了，用来瞄准不够 —— 瞄准靠每次探测后的闭环修正。
pub fn wood_dist(tier: i32) -> Option<f32> {
    match tier {
        ..=0 => None,
        1..=4 => Some(3.0),
        5..=8 => Some(6.0),
        9..=16 => Some(12.0),
        17.. => Some(24.0),
    }
}

/// 数据包报的矿石距离档 -> 格数。档位 `[8, 4, 2]`。
///
/// 比原木收得近得多：地下根本没有"远处有矿"这回事，8 格以外全是石头。
/// 报远了只会让 AI 朝着一堵石墙走过去。
pub fn ore_dist(tier: i32) -> Option<f32> {
    match tier {
        ..=0 => None,
        1..=2 => Some(2.0),
        3..=4 => Some(4.0),
        5..=8 => Some(8.0),
        _ => Some(12.0),
    }
}

/// 采集三个阶段的瞄准角（度）。MC 的 pitch 正数往下看。
///
/// 砍树瞄 +22° 是有讲究的：树干在眼高以下的那一两格才是"站着伸手就能够到"
/// 的，瞄太高会砍到树叶，瞄太低会砍到地面。上面几格够不着就够不着 ——
/// 真人砍树也是砍断下面两三格就走，不跳着把整棵削平。
pub const PITCH_CHOP: f32 = 22.0;
pub const PITCH_WALK: f32 = 0.0;

/// 死区：角度差小于这个值就不动视线。
///
/// 没有死区的话，`Tri` 只给方向，每一拍都投一次 `Pos` 会一路转到脚面。
/// 6° 大约是一个方块在 3 格外的张角，够用且不抖。
pub const AIM_DEADBAND: f32 = 6.0;

/// 眼睛离脚底的高度。算瞄准角要用。
const EYE_HEIGHT: f32 = 1.62;

/// 瞄准点取在脚底上方多高。
///
/// 一格方块的上表面在脚底 +1.0；挖矿时目标往往低一格到两格（挖矿道往下走的
/// 时候），取 0.42 当折中。于是视线落差 = `EYE_HEIGHT - AIM_TARGET_Y ≈ 1.2`。
const AIM_TARGET_Y: f32 = 0.42;

/// 挖矿时该把视线压到多少度。
///
/// 砍树可以写死 22°，因为树干总在差不多的位置；挖矿不行 —— 站着挖脚边和
/// 隔着三格挖矿道，需要的角度差一倍。所以按距离算：
/// `atan(落差 / 水平距离)`，目标越近角度越大。
pub fn aim_pitch(dist: f32) -> f32 {
    let d = dist.max(0.8);
    let drop = EYE_HEIGHT - AIM_TARGET_Y;
    (drop / d).atan().to_degrees().clamp(10.0, 72.0)
}

/// 把「当前俯仰角 → 目标角」翻译成三值方向。
///
/// 返回 `None` = 已经对准了，**不要投票**。注意是"不投票"而不是"投弃权"：
/// 裁决器里只有本拍没人投票才会松手，投弃权压不掉上一拍的 `Pos`（迟滞）。
pub fn aim(cur: Option<f32>, want: f32) -> Option<Tri> {
    let cur = cur?;
    let diff = want - cur;
    if diff.abs() <= AIM_DEADBAND {
        None
    } else if diff > 0.0 {
        // 目标角更大 = 要更往下看
        Some(Tri::Pos)
    } else {
        Some(Tri::Neg)
    }
}

/// 多少次连续探测不到目标，才算"采掉了一个"。
///
/// 不能只认一次：树干的判定点在眼睛高度，走到树根底下时那格可能是木头也可能是
/// 空气（树干只有 1 格宽），偶尔会抖一下报"没有"。两次采样都空才认。
const MISS_CONFIRM: u32 = 2;

pub struct Gather {
    pub mode: Mode,
    /// 本次连采已采几个（够 `quota` 就去休息）
    pub chopped: u32,
    /// 被危险打断了几次。这个数一直涨说明这个位置根本不适合采集
    pub interrupted: u32,
    /// 连续安全的起点。`None` = 当前不安全，窗口重头计
    safe_since: Option<u64>,
    /// 惩罚冷却到期时刻
    cd_until: u64,
    /// 配额满了以后的休息到期时刻
    rest_until: u64,
    /// 下次插播扫视的时刻 / 本次扫视结束时刻
    scan_at: u64,
    scan_until: u64,
    scan_side: Tri,
    /// 「目标可能采完了」的候选状态。
    ///
    /// 不能在动手模式下直接数缺口：目标一消失，门控立刻判 `NoWood` 把模式翻成
    /// `Off`，第二拍就进不了计数分支，永远等不到确认。所以要有一个跨模式的
    /// 候选标记。
    pending_fell: bool,
    fell_streak: u32,
    /// 开始动手的时刻。
    ///
    /// 用 `Option` 而不是 0 当哨兵：`now_ms` 真的可以是 0（sim 和测试都从 0
    /// 开始计时）。用 0 当"还没开始"，第一拍就会把 `Some(0)` 当成 `None`
    /// 重新计时，`max_chop_ms` 永远触发不了 —— 这个坑在 `pos_win_start`
    /// 和 `squad.attempted` 上已经踩过两次了。
    chop_since: Option<u64>,
    /// 这一拍的门控结论，给日志用
    pub gate: Tri,
    pub why: Why,
    /// 上一次冷却是因为什么。`why` 每拍重算，冷却期间会一直是 `cd`，
    /// 想知道"到底被什么打断的"就得看这个。
    pub interrupted_by: Why,
    /// 最近一次决策用了哪个距离（给行为层拿去转向，避免重复读世界）
    pub target_dist: Option<f32>,
    /// 最近一次决策瞄的目标方块
    pub target_pitch: f32,
    /// 最近一次「墙壁判据」的结论，给 `!gather` 看
    pub wall_note: &'static str,
}

impl Default for Gather {
    fn default() -> Self {
        Gather::new()
    }
}

impl Gather {
    pub fn new() -> Gather {
        Gather {
            mode: Mode::Off,
            chopped: 0,
            interrupted: 0,
            safe_since: None,
            cd_until: 0,
            rest_until: 0,
            scan_at: 0,
            scan_until: 0,
            scan_side: Tri::Pos,
            pending_fell: false,
            fell_streak: 0,
            chop_since: None,
            gate: Tri::Neg,
            why: Why::Disabled,
            interrupted_by: Why::Ok,
            target_dist: None,
            target_pitch: PITCH_CHOP,
            wall_note: "-",
        }
    }

    pub fn line(&self, cfg: &GatherCfg) -> String {
        format!(
            "gather={} 目标={} why={} 采={} 断={}({}) 墙={}",
            self.mode.name(),
            cfg.target.name(),
            self.why.as_str(),
            self.chopped,
            self.interrupted,
            self.interrupted_by.as_str(),
            self.wall_note
        )
    }

    /// 手动重置：`!gather reset` 清配额和冷却，让它现在立刻可以开工。
    ///
    /// 连 `pending_fell` 一起清 —— 不清的话，如果正好卡在"目标没了但还没确认"
    /// 那一拍，reset 之后第一次 tick 会立刻多算一个，配额莫名其妙少一格。
    pub fn reset(&mut self) {
        self.chopped = 0;
        self.interrupted = 0;
        self.cd_until = 0;
        self.rest_until = 0;
        self.safe_since = None;
        self.pending_fell = false;
        self.fell_streak = 0;
        self.chop_since = None;
        self.scan_until = 0;
        self.scan_at = 0;
        self.mode = Mode::Off;
        self.why = Why::Ok;
        self.interrupted_by = Why::Ok;
    }

    /// 门控：三值结论 + 原因。纯函数，不推进状态 —— 这样行为层可以反复问。
    pub fn gate(&self, w: &WorldView, cfg: &GatherCfg, now_ms: u64) -> (Tri, Why) {
        // 配置关着 / 休息中：明确不许
        if !cfg.enabled {
            return (Tri::Neg, Why::Disabled);
        }
        if now_ms < self.cd_until {
            return (Tri::Neg, Why::Cooldown);
        }
        if now_ms < self.rest_until {
            return (Tri::Neg, Why::QuotaDone);
        }

        // 1. 威胁。这条排最前，因为它变化最快、后果最重。
        //    `Zero` = 不确定（传感器没报 / 刚死过）→ 也不开工。采集是"有余力
        //    才干"的事，任何不确定都该让位。
        match w.threat {
            Tri::Pos => return (Tri::Neg, Why::Threat),
            Tri::Zero => return (Tri::Zero, Why::ThreatUnknown),
            Tri::Neg => {}
        }

        // 2. 血量。砍树时背对着树、视线朝下，挨一下很难受。
        match w.hp {
            None => return (Tri::Zero, Why::HpUnknown),
            Some(h) if h < cfg.min_hp => return (Tri::Neg, Why::LowHp),
            // 临界带：还能砍，但下一拍被打就危险，先判不确定
            Some(h) if h < cfg.min_hp + 0.15 => return (Tri::Zero, Why::HpMarginal),
            Some(_) => {}
        }

        // 3. 饱食度。饿着砍树是最亏的：砍完跑不动，而且不能回血。
        match w.food {
            None => return (Tri::Zero, Why::FoodUnknown),
            Some(f) if f <= cfg.min_food => return (Tri::Neg, Why::Hungry),
            Some(_) => {}
        }

        // 4. 爆炸源。`Zero` 在这里是常态（数据包没报爆炸物 = 安全），只有 Pos 才躲。
        if w.hazard == Tri::Pos {
            return (Tri::Neg, Why::Hazard);
        }

        // 5. 兜底：威胁判据来自数据包，可能滞后；距离是独立通道，取并集。
        if let Some(d) = w.dist {
            if d <= ENEMY_CLEAR {
                return (Tri::Neg, Why::EnemyNear);
            }
        }

        // 6. 昼夜。夜里刷怪密度高好几倍，默认不干。
        //    不知道时间时按"夜里"处理 —— 保守方向。
        match w.night {
            Some(true) if !cfg.allow_night => return (Tri::Neg, Why::Night),
            Some(true) => {}
            None if !cfg.allow_night => return (Tri::Zero, Why::NightUnknown),
            _ => {}
        }

        // 7. 有没有目标。`None` = 视野内没有（数据包射线全没命中）。
        let d = match cfg.target.dist(w) {
            None => {
                return (
                    Tri::Neg,
                    match cfg.target {
                        Target::Wood => Why::NoWood,
                        Target::Ore => Why::NoOre,
                    },
                )
            }
            Some(d) => d,
        };
        if d > cfg.max_walk {
            // 看得见但太远：走过去的路上风险不可控，不去
            return (Tri::Neg, Why::TooFar);
        }

        // 8. 挖矿独有的地形判据 —— 砍树不需要，因为树不会把人埋了。
        if cfg.target == Target::Ore {
            // 8a. 目标本身是岩浆就绝对不挖。矿石 tag 和危险 tag 是两套判据，
            //     必须先看危险：`blk_front` 只报一个类别，而岩浆在 `hazard`
            //     和 `lava` 里都可能出现。
            if matches!(w.blk_front, Some(Block::Lava) | Some(Block::Hazard)) {
                return (Tri::Neg, Why::Lava);
            }
            // 8b. 脚下要有支撑。悬空挖矿 = 掉进自己刚挖的坑。
            //     方块数据没到（快通道没开）时不是"没支撑"，是"不知道" ——
            //     这条最容易写错：把 None 当成 Empty 会让 AI 一下矿道就罢工。
            match w.blk_below {
                None => return (Tri::Zero, Why::NoBlockData),
                Some(Block::Empty) => return (Tri::Neg, Why::NoFloor),
                Some(Block::Lava) | Some(Block::Hazard) => return (Tri::Neg, Why::Lava),
                Some(Block::Water) => return (Tri::Neg, Why::NoFloor),
                Some(_) => {}
            }
        }

        // 9. 配额。采够了就走，别把整片林子/整条矿脉推平 ——
        //    也给自己一个"回城"的节奏。
        //    注意 `quota = 0` 是「不限制」。少了 `> 0` 这个条件，`chopped >= 0`
        //    恒真，等于把采集永久锁死在"配额已完成"。
        if cfg.quota > 0 && self.chopped >= cfg.quota {
            return (Tri::Neg, Why::QuotaDone);
        }

        (Tri::Pos, Why::Ok)
    }

    /// 「正前方有没有墙」的独立判据。
    ///
    /// 不进 `gate`：墙上不挡砍树（你站在树旁边砍，前面有墙无所谓），
    /// 只挡**走过去**那一步。塞进 gate 会让 mode 在 `Approach` 里反复翻
    /// `Blocked`，看起来像抽风。
    ///
    /// 返回 `None` = 没有墙体数据（快通道没开），不是"没墙"。
    pub fn path_blocked(&self, w: &WorldView, cfg: &GatherCfg) -> Option<bool> {
        let wall = w.wall?;
        // 目标方向上有墙、而准星正对的又不能挖，才算挡路。
        // 否则挖矿时会自己把自己拦下：矿石本来就是固体，wall 位一定是 1。
        if matches!(w.blk_front, Some(Block::Ore) | Some(Block::Log)) {
            return Some(false);
        }
        // 一格高的台阶不算挡路 —— 跳上去就完了。`step` 是 None（老数据包）
        // 时按"挡"处理，因为那时代码里根本没有跳过台阶的分支，
        // 强行放行只会让 AI 一头撞上去。
        if w.step == Some(true) {
            return Some(false);
        }
        let dir = cfg.target.dir(w).unwrap_or(0);
        // 直走方向和目标方向都可能被挡；两个都查一遍
        let ahead = crate::perceive::wall_at(wall, 0);
        let toward = crate::perceive::wall_at(wall, dir.clamp(-1, 1));
        Some(ahead || toward)
    }

    /// 每拍推进一次。返回值决定行为层这一拍做什么。
    pub fn tick(&mut self, w: &WorldView, cfg: &GatherCfg, now_ms: u64) -> Mode {
        let (g, why) = self.gate(w, cfg, now_ms);
        self.gate = g;
        self.why = why;
        self.target_dist = cfg.target.dist(w);
        self.target_pitch = match cfg.target {
            Target::Wood => PITCH_CHOP,
            Target::Ore => aim_pitch(self.target_dist.unwrap_or(3.0)),
        };
        let want = cfg.target.dist(w);

        // ---- 记录"采掉了一个" ----
        // 判据：上一拍正在动手，这一拍目标没了。缺失要连续两拍才认。
        if self.mode.holding() && want.is_none() {
            self.pending_fell = true;
        }
        if want.is_some() {
            // 又看见了 → 刚才只是采样抖动
            self.pending_fell = false;
            self.fell_streak = 0;
        } else if self.pending_fell {
            self.fell_streak += 1;
            if self.fell_streak >= MISS_CONFIRM {
                self.chopped += 1;
                self.pending_fell = false;
                self.fell_streak = 0;
                self.chop_since = None;
            }
        }

        // ---- 配额满了就休息，休息完自动开下一趟 ----
        if cfg.quota > 0 && self.chopped >= cfg.quota && self.rest_until == 0 {
            self.rest_until = now_ms + cfg.rest_ms;
            self.mode = Mode::Off;
            return self.mode;
        }
        if self.rest_until > 0 && now_ms >= self.rest_until {
            // 新的一趟
            self.chopped = 0;
            self.rest_until = 0;
            self.safe_since = None;
        }

        match g {
            // 明确不许：收手。如果是"危险"类原因，吃惩罚冷却。
            Tri::Neg => {
                let was_working = matches!(
                    self.mode,
                    Mode::Approach | Mode::Chop | Mode::Mine | Mode::Scan
                );
                if was_working && why.is_unsafe() {
                    self.interrupted += 1;
                    self.cd_until = now_ms + cfg.cooldown_ms;
                    self.interrupted_by = why;
                }
                self.safe_since = None;
                self.scan_until = 0;
                self.chop_since = None;
                self.mode = if now_ms < self.cd_until {
                    Mode::Cooldown
                } else {
                    Mode::Off
                };
            }
            // 不确定：站住不动，安全窗口重头计。注意这里**不**吃冷却 ——
            // 不确定不是危险，不该罚。
            Tri::Zero => {
                self.safe_since = None;
                self.scan_until = 0;
                self.chop_since = None;
                self.mode = Mode::Wait;
            }
            Tri::Pos => {
                let since = *self.safe_since.get_or_insert(now_ms);
                if now_ms.saturating_sub(since) < cfg.enter_ms {
                    // 安全窗口还没攒够。`why` 从 `ok` 改成 `warmup` ——
                    // 否则日志里会显示"安全但什么都没干"，看起来像卡住了。
                    self.why = Why::Warmup;
                    self.mode = Mode::Wait;
                    return self.mode;
                }

                // ---- 采集中的插播扫视：眼观六路 ----
                if cfg.scan_every_ms > 0 && now_ms >= self.scan_at {
                    self.scan_until = now_ms + cfg.scan_ms;
                    self.scan_at = now_ms + cfg.scan_every_ms;
                    self.scan_side = self.scan_side.not();
                }
                if now_ms < self.scan_until {
                    self.mode = Mode::Scan;
                    return self.mode;
                }

                let d = want.unwrap_or(f32::INFINITY);

                // 走/挖太久还没成 → 判断错了。别在那儿杵着，进冷却换个地方。
                if let (true, Some(since)) = (self.mode.holding(), self.chop_since) {
                    if now_ms.saturating_sub(since) > cfg.max_chop_ms {
                        self.interrupted += 1;
                        self.cd_until = now_ms + cfg.cooldown_ms;
                        self.chop_since = None;
                        self.why = Why::LostTree;
                        self.interrupted_by = Why::LostTree;
                        self.fell_streak = 0;
                        self.pending_fell = false;
                        self.mode = Mode::Cooldown;
                        return self.mode;
                    }
                }

                if d <= cfg.reach {
                    // 只在「第一次进入动手」时起表。扫视会把 mode 改成 Scan 再
                    // 改回来，如果按 mode 变化重置，插播一次就重新计时，
                    // `max_chop_ms` 永远触发不了 —— 卡住的目标能挖一整天。
                    self.chop_since.get_or_insert(now_ms);
                    self.mode = match cfg.target {
                        Target::Wood => Mode::Chop,
                        Target::Ore => Mode::Mine,
                    };
                } else {
                    // 走不过去就别硬走：地上能绕，矿道里对着墙走十秒
                    // 和"卡死"是同一个观感。
                    if self.path_blocked(w, cfg) == Some(true) {
                        self.wall_note = "挡";
                        self.chop_since = None;
                        self.why = Why::Blocked;
                        self.mode = Mode::Wait;
                        return self.mode;
                    }
                    self.wall_note = if self.path_blocked(w, cfg).is_some() {
                        "通"
                    } else {
                        "?"
                    };
                    self.chop_since = None;
                    self.mode = Mode::Approach;
                }
            }
        }
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认配置是关的（采集会改变世界，不该默认开）。测试里要显式打开，
    /// 剩下的参数沿用默认值。
    fn cfg() -> GatherCfg {
        GatherCfg {
            enabled: true,
            // 扫视单独测。混在别的用例里会让"最后一拍的 mode"变得不可预测。
            scan_every_ms: 0,
            ..Default::default()
        }
    }

    fn ore_cfg() -> GatherCfg {
        GatherCfg {
            target: Target::Ore,
            ..cfg()
        }
    }

    /// 一个「明确安全 + 有木头」的世界
    fn safe(wood: Option<f32>) -> WorldView {
        WorldView {
            hp: Some(1.0),
            food: Some(18),
            threat: Tri::Neg,
            dist: None,
            hazard: Tri::Zero,
            wood,
            wood_dir: Some(0),
            night: Some(false),
            ..Default::default()
        }
    }

    /// 一个「明确安全 + 前方有矿 + 脚下是石头」的世界
    fn mine(ore: Option<f32>) -> WorldView {
        WorldView {
            ore,
            ore_dir: Some(0),
            blk_front: Some(Block::Ore),
            blk_below: Some(Block::Solid),
            wall: Some(0),
            ..safe(None)
        }
    }

    /// 连续推到开工（从 t=0 开始）。
    fn warm(g: &mut Gather, w: &WorldView, c: &GatherCfg) -> Mode {
        warm_at(g, w, c, 0)
    }

    /// 从指定时刻开始连续推进 6 秒。时间不能倒退 —— 冷却和窗口都是绝对时刻。
    fn warm_at(g: &mut Gather, w: &WorldView, c: &GatherCfg, start: u64) -> Mode {
        let mut m = Mode::Off;
        for t in (start..start + 6_000).step_by(100) {
            m = g.tick(w, c, t);
        }
        m
    }

    #[test]
    fn disabled_never_works() {
        let c = GatherCfg {
            enabled: false,
            ..cfg()
        };
        let mut g = Gather::new();
        assert_eq!(warm(&mut g, &safe(Some(4.0)), &c), Mode::Off);
        assert_eq!(g.gate, Tri::Neg);
        assert_eq!(g.why, Why::Disabled);
    }

    #[test]
    fn walks_then_chops_then_counts_one_tree() {
        let c = cfg();
        let mut g = Gather::new();
        // 隔着 8 格：先走过去
        assert_eq!(warm(&mut g, &safe(Some(8.0)), &c), Mode::Approach);
        // 到了 2 格：开砍
        assert_eq!(warm(&mut g, &safe(Some(2.0)), &c), Mode::Chop);
        // 木头消失两次 = 砍倒一棵
        warm(&mut g, &safe(None), &c);
        warm(&mut g, &safe(None), &c);
        assert_eq!(g.chopped, 1);
    }

    #[test]
    fn one_missing_sample_is_not_a_felled_tree() {
        let c = cfg();
        let mut g = Gather::new();
        warm(&mut g, &safe(Some(2.0)), &c);
        g.tick(&safe(None), &c, 10_000);
        g.tick(&safe(Some(2.0)), &c, 10_100);
        assert_eq!(g.chopped, 0);
    }

    #[test]
    fn threat_stops_immediately_and_costs_cooldown() {
        let c = cfg();
        let mut g = Gather::new();
        assert_eq!(warm(&mut g, &safe(Some(2.0)), &c), Mode::Chop);

        let mut w = safe(Some(2.0));
        w.threat = Tri::Pos;
        assert_eq!(g.tick(&w, &c, 7_000), Mode::Cooldown);
        assert_eq!(g.interrupted, 1);

        // 威胁走了也要等冷却，不能怪物一转背就回去砍
        assert_eq!(g.tick(&safe(Some(2.0)), &c, 7_500), Mode::Cooldown);
        assert_eq!(g.interrupted_by, Why::Threat);
        // 冷却过了才回
        assert_eq!(warm_at(&mut g, &safe(Some(2.0)), &c, 16_000), Mode::Chop);
    }

    #[test]
    fn unknown_threat_backs_off_without_punishment() {
        let c = cfg();
        let mut g = Gather::new();
        warm(&mut g, &safe(Some(2.0)), &c);
        let mut w = safe(Some(2.0));
        w.threat = Tri::Zero;
        assert_eq!(g.tick(&w, &c, 7_000), Mode::Wait);
        assert_eq!(g.interrupted, 0);
        assert_eq!(g.why, Why::ThreatUnknown);
    }

    #[test]
    fn entry_needs_a_quiet_window() {
        let c = cfg();
        let mut g = Gather::new();
        assert_eq!(g.tick(&safe(Some(2.0)), &c, 0), Mode::Wait);
        assert_eq!(g.tick(&safe(Some(2.0)), &c, c.enter_ms - 100), Mode::Wait);
        assert!(g.tick(&safe(Some(2.0)), &c, c.enter_ms + 100).active());
    }

    #[test]
    fn window_resets_when_it_blinks() {
        let c = cfg();
        let mut g = Gather::new();
        g.tick(&safe(Some(2.0)), &c, 0);
        g.tick(&safe(Some(2.0)), &c, 1_000);
        let mut w = safe(Some(2.0));
        w.threat = Tri::Zero;
        g.tick(&w, &c, 1_500);
        assert_eq!(g.tick(&safe(Some(2.0)), &c, 2_000), Mode::Wait);
        assert_eq!(g.tick(&safe(Some(2.0)), &c, 3_000), Mode::Wait);
        assert!(g.tick(&safe(Some(2.0)), &c, 2_000 + c.enter_ms + 1).active());
    }

    #[test]
    fn low_hp_refuses() {
        let c = cfg();
        let mut g = Gather::new();
        let mut w = safe(Some(2.0));
        w.hp = Some(0.3);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::LowHp);
        assert_eq!(g.mode, Mode::Off);
    }

    #[test]
    fn hungry_refuses() {
        let c = cfg();
        let mut g = Gather::new();
        let mut w = safe(Some(2.0));
        w.food = Some(3);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::Hungry);
    }

    #[test]
    fn enemy_within_16_blocks_blocks_work() {
        let c = cfg();
        let mut g = Gather::new();
        let mut w = safe(Some(2.0));
        w.dist = Some(12.0);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::EnemyNear);
    }

    #[test]
    fn night_off_by_default_unknown_counts_as_night() {
        let c = cfg();
        let mut g = Gather::new();
        let mut w = safe(Some(2.0));
        w.night = Some(true);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::Night);

        let mut w = safe(Some(2.0));
        w.night = None;
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::NightUnknown);
        assert_eq!(g.mode, Mode::Wait);
    }

    #[test]
    fn no_wood_is_a_reason_not_a_crash() {
        let c = cfg();
        let mut g = Gather::new();
        warm(&mut g, &safe(None), &c);
        assert_eq!(g.why, Why::NoWood);
        assert_eq!(g.mode, Mode::Off);
    }

    #[test]
    fn too_far_is_refused() {
        let c = cfg();
        let mut g = Gather::new();
        warm(&mut g, &safe(Some(40.0)), &c);
        assert_eq!(g.why, Why::TooFar);
    }

    #[test]
    fn quota_then_rest_then_new_trip() {
        let c = GatherCfg {
            quota: 2,
            rest_ms: 5_000,
            ..cfg()
        };
        let mut g = Gather::new();
        g.chopped = 2;
        let mut t = 0;
        while t < 4_000 {
            g.tick(&safe(Some(2.0)), &c, t);
            t += 100;
        }
        assert_eq!(g.mode, Mode::Off);
        assert_eq!(g.why, Why::QuotaDone);
        for t in (4_000..12_000).step_by(100) {
            g.tick(&safe(Some(2.0)), &c, t);
        }
        assert_eq!(g.chopped, 0);
        assert!(g.mode.active() || g.mode == Mode::Scan, "{:?}", g.mode);
    }

    #[test]
    fn periodic_scan_interrupts_chopping() {
        let c = GatherCfg {
            scan_every_ms: 1_000,
            scan_ms: 300,
            ..cfg()
        };
        let mut g = Gather::new();
        warm(&mut g, &safe(Some(2.0)), &c);
        let mut saw_scan = false;
        for t in (6_000..9_000).step_by(50) {
            if g.tick(&safe(Some(2.0)), &c, t) == Mode::Scan {
                saw_scan = true;
            }
        }
        assert!(saw_scan, "扫视窗口没触发，等于闷头砍树");
    }

    #[test]
    fn chopping_too_long_gives_up() {
        let c = GatherCfg {
            max_chop_ms: 2_000,
            ..cfg()
        };
        let mut g = Gather::new();
        warm(&mut g, &safe(Some(2.0)), &c);
        let mut last = Mode::Off;
        for t in (6_000..12_000).step_by(50) {
            last = g.tick(&safe(Some(2.0)), &c, t);
        }
        assert_eq!(g.interrupted_by, Why::LostTree);
        assert_eq!(last, Mode::Cooldown);
    }

    #[test]
    fn reset_clears_everything() {
        let c = cfg();
        let mut g = Gather::new();
        g.chopped = 9;
        g.cd_until = 99_999;
        g.reset();
        assert_eq!(g.chopped, 0);
        assert!(warm(&mut g, &safe(Some(2.0)), &c).active());
    }

    #[test]
    fn aim_turns_until_on_target_then_stops_voting() {
        assert_eq!(aim(Some(0.0), 22.0), Some(Tri::Pos), "要往下看");
        assert_eq!(aim(Some(40.0), 22.0), Some(Tri::Neg), "要往上看");
        assert_eq!(aim(Some(22.0), 22.0), None);
        assert_eq!(aim(Some(17.0), 22.0), None);
        assert_eq!(aim(Some(27.0), 22.0), None);
        assert_eq!(aim(None, 22.0), None);
    }

    #[test]
    fn wood_tier_maps_to_distance() {
        assert_eq!(wood_dist(0), None);
        assert_eq!(wood_dist(3), Some(3.0));
        assert_eq!(wood_dist(6), Some(6.0));
        assert_eq!(wood_dist(12), Some(12.0));
    }

    #[test]
    fn quota_zero_means_unlimited_not_already_done() {
        let c = GatherCfg { quota: 0, ..cfg() };
        let mut g = Gather::new();
        g.chopped = 99;
        assert!(warm(&mut g, &safe(Some(2.0)), &c).active());
    }

    #[test]
    fn chop_timer_works_from_time_zero() {
        let c = GatherCfg {
            max_chop_ms: 1_500,
            enter_ms: 0,
            ..cfg()
        };
        let mut g = Gather::new();
        let mut last = Mode::Off;
        for t in (0..4_000).step_by(50) {
            last = g.tick(&safe(Some(2.0)), &c, t);
        }
        assert_eq!(g.interrupted_by, Why::LostTree, "从 t=0 起砍也必须能超时");
        assert_eq!(last, Mode::Cooldown);
    }

    #[test]
    fn reset_clears_the_pending_fell_marker() {
        let c = cfg();
        let mut g = Gather::new();
        warm(&mut g, &safe(Some(2.0)), &c);
        g.tick(&safe(None), &c, 10_000); // 第一拍缺失 → pending
        g.reset();
        g.tick(&safe(None), &c, 10_500);
        assert_eq!(g.chopped, 0, "reset 之后不该凭空多一棵");
    }

    #[test]
    fn reasons_are_classified_correctly() {
        assert!(Why::Threat.is_unsafe());
        assert!(Why::EnemyNear.is_unsafe());
        assert!(!Why::NoWood.is_unsafe());
        assert!(!Why::QuotaDone.is_unsafe());
        assert!(!Why::Night.is_unsafe(), "夜里不该算被打断");
        assert!(Why::Lava.is_unsafe(), "挖到岩浆必须算危险");
        assert!(Why::NoFloor.is_unsafe(), "掉坑也算危险");
        assert!(!Why::Blocked.is_unsafe(), "被墙挡只是走不通，不是危险");
    }

    // ---- 挖矿 ----

    #[test]
    fn ore_tier_maps_to_distance() {
        assert_eq!(ore_dist(0), None);
        assert_eq!(ore_dist(2), Some(2.0));
        assert_eq!(ore_dist(4), Some(4.0));
        assert_eq!(ore_dist(8), Some(8.0));
    }

    #[test]
    fn mining_enters_mine_mode_not_chop() {
        let c = ore_cfg();
        let mut g = Gather::new();
        assert_eq!(warm(&mut g, &mine(Some(2.0)), &c), Mode::Mine);
        assert!(g.mode.holding(), "挖矿要按住左键");
    }

    #[test]
    fn wood_target_ignores_ore() {
        // 目标是木头时，看得见矿也不去挖
        let c = cfg();
        let mut g = Gather::new();
        let mut w = mine(Some(2.0));
        w.wood = None;
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::NoWood);
    }

    #[test]
    fn ore_target_ignores_wood() {
        let c = ore_cfg();
        let mut g = Gather::new();
        let mut w = mine(None);
        w.wood = Some(2.0);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::NoOre);
    }

    #[test]
    fn no_floor_refuses_to_mine() {
        // 脚下一空，挖下去就是掉进自己刚挖的坑
        let c = ore_cfg();
        let mut g = Gather::new();
        let mut w = mine(Some(2.0));
        w.blk_below = Some(Block::Empty);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::NoFloor);
        assert_eq!(g.mode, Mode::Off);
    }

    #[test]
    fn standing_on_lava_refuses() {
        let c = ore_cfg();
        let mut g = Gather::new();
        let mut w = mine(Some(2.0));
        w.blk_below = Some(Block::Lava);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::Lava);
    }

    #[test]
    fn standing_in_water_refuses() {
        // 水里挖矿 = 挖开就淹，而且游着够不到方块
        let c = ore_cfg();
        let mut g = Gather::new();
        let mut w = mine(Some(2.0));
        w.blk_below = Some(Block::Water);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::NoFloor);
    }

    #[test]
    fn aiming_at_lava_refuses_even_if_ore_nearby() {
        // 矿就在旁边，但准星对着岩浆 —— 先看危险，再看收益
        let c = ore_cfg();
        let mut g = Gather::new();
        let mut w = mine(Some(2.0));
        w.blk_front = Some(Block::Lava);
        warm(&mut g, &w, &c);
        assert_eq!(g.why, Why::Lava);
    }

    #[test]
    fn missing_block_data_is_unknown_not_unsafe() {
        // 回归：把 blk_below 的 None 当成 Empty 会让 AI 一下矿道就永久罢工。
        // 它该是「不确定」—— 站住等，但不吃惩罚冷却。
        let c = ore_cfg();
        let mut g = Gather::new();
        let mut w = mine(Some(2.0));
        w.blk_below = None;
        for t in (0..6_000).step_by(100) {
            g.tick(&w, &c, t);
        }
        assert_eq!(g.why, Why::NoBlockData);
        assert_eq!(g.gate, Tri::Zero);
        assert_eq!(g.mode, Mode::Wait);
        assert_eq!(g.interrupted, 0, "不知道不该算被打断");
    }

    #[test]
    fn mining_counts_one_ore_when_it_disappears() {
        let c = ore_cfg();
        let mut g = Gather::new();
        warm(&mut g, &mine(Some(2.0)), &c);
        assert_eq!(g.mode, Mode::Mine);
        warm(&mut g, &mine(None), &c);
        warm(&mut g, &mine(None), &c);
        assert_eq!(g.chopped, 1);
    }

    #[test]
    fn wall_blocks_approach_but_not_chopping() {
        // 前方有墙、又没什么可挖的 → 走过去那一步被拦
        let c = cfg();
        let mut g = Gather::new();
        let mut w = safe(Some(8.0));
        w.wall = Some(1); // 正前方 1 格是固体
        w.blk_front = Some(Block::Solid);
        w.step = Some(false); // 而且上面也是固体 → 两格高，跳不过去
        for t in (0..6_000).step_by(100) {
            g.tick(&w, &c, t);
        }
        assert_eq!(g.why, Why::Blocked);
        assert_eq!(g.mode, Mode::Wait);
        assert_eq!(g.wall_note, "挡");
    }

    #[test]
    fn ore_ahead_is_not_a_wall() {
        // 挖矿时正前方就是矿石 —— 它当然是固体，但不能算"被墙挡住"，
        // 否则 AI 会自己把自己拦下来，永远不开始挖。
        let c = ore_cfg();
        let mut g = Gather::new();
        let mut w = mine(Some(8.0));
        w.wall = Some(1);
        w.blk_front = Some(Block::Ore);
        for t in (0..6_000).step_by(100) {
            g.tick(&w, &c, t);
        }
        assert_eq!(g.mode, Mode::Approach, "why={:?}", g.why);
    }

    #[test]
    fn no_wall_data_does_not_block() {
        // 快通道没开时 wall 是 None —— 不能因此罢工，只是不知道而已
        let c = cfg();
        let mut g = Gather::new();
        let mut w = safe(Some(8.0));
        w.wall = None;
        w.blk_front = None;
        warm(&mut g, &w, &c);
        assert_eq!(g.mode, Mode::Approach);
        assert_eq!(g.wall_note, "?");
    }

    #[test]
    fn mining_pitch_follows_distance() {
        // 越近越要往下看。写死一个角度的话，脚边的矿会永远瞄不到。
        assert!(aim_pitch(1.0) > aim_pitch(2.0));
        assert!(aim_pitch(2.0) > aim_pitch(4.0));
        assert!(aim_pitch(0.2) <= 72.0, "不能转到正下方");
        assert!(aim_pitch(99.0) >= 10.0, "远处也得低头");
        // 3 格外的矿石大约 22°，和砍树的角度接近
        assert!((aim_pitch(3.0) - 21.8).abs() < 2.0);
    }

    #[test]
    fn target_parsing() {
        assert_eq!(Target::parse("ore"), Some(Target::Ore));
        assert_eq!(Target::parse("矿"), Some(Target::Ore));
        assert_eq!(Target::parse("wood"), Some(Target::Wood));
        assert_eq!(Target::parse("树"), Some(Target::Wood));
        assert_eq!(Target::parse("nope"), None);
    }
}
