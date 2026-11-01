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

use crate::config::GatherCfg;
use crate::fsm::WorldView;
use crate::trit::Tri;

/// 采集过程中的内部模式。对外只用于观察，不参与通道裁决。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// 没在采集（配置关着，或配额已满在休息）
    Off,
    /// 想采，但安全窗口还没攒够 —— 站着等，不动手
    Wait,
    /// 朝木头走过去
    Approach,
    /// 正在砍
    Chop,
    /// 采集中的插播：停手抬头看周围
    Scan,
    /// 被打断了，歇一会儿
    Cooldown,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Wait => "wait",
            Mode::Approach => "walk",
            Mode::Chop => "chop",
            Mode::Scan => "scan",
            Mode::Cooldown => "cd",
        }
    }
    /// 这个模式下 AI 是否该真的动手（走 / 砍）
    pub fn active(self) -> bool {
        matches!(self, Mode::Approach | Mode::Chop)
    }
}

/// 「为什么要停工」的机器可读原因。给 `!gather` 和 sim 用。
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
    TooFar,
    /// 砍太久还没断，多半判断错了（树在墙后 / 在坡上）
    LostTree,
    /// 明确安全，但「连续安全」的窗口还没攒够 —— 站着数秒
    Warmup,
    Ok,
}

impl Why {
    pub fn as_str(self) -> &'static str {
        match self {
            Why::Disabled => "off",
            Why::QuotaDone => "quota",
            Why::Cooldown => "cd",
            Why::Threat => "threat",
            Why::ThreatUnknown => "threat?",
            Why::LowHp => "low_hp",
            Why::HpMarginal => "hp?",
            Why::HpUnknown => "hp?",
            Why::Hungry => "hungry",
            Why::FoodUnknown => "food?",
            Why::Hazard => "hazard",
            Why::EnemyNear => "enemy_near",
            Why::Night => "night",
            Why::NightUnknown => "night?",
            Why::NoWood => "no_wood",
            Why::TooFar => "too_far",
            Why::LostTree => "lost",
            Why::Warmup => "warmup",
            Why::Ok => "ok",
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
/// 档位是 `[12, 6, 3]`（见 `scripts/make_trit_bridge.py`）。`0` = 8 个方向
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

/// 采集三个阶段的瞄准角（度）。MC 的 pitch 正数往下看。
///
/// 砍树瞄 +22° 是有讲究的：树干在眼高以下的那一两格才是"站着伸手就能够到"
/// 的，瞄太高会砍到树叶，瞄太低会砍到地面。上面几格够不着就够不着 ——
/// 真人砍树也是砍断下面两三格就走，不跳着把整棵削平。
pub const PITCH_CHOP: f32 = 22.0;
pub const PITCH_WALK: f32 = 0.0;
pub const PITCH_SCAN: f32 = -8.0;

/// 死区：角度差小于这个值就不动视线。
///
/// 没有死区的话，`Tri` 只给方向，每一拍都投一次 `Pos` 会一路转到脚面。
/// 6° 大约是一个方块在 3 格外的张角，够用且不抖。
pub const AIM_DEADBAND: f32 = 6.0;

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

/// 多少次连续探测不到木头，才算"砍掉了一棵"。
///
/// 不能只认一次：树干的判定点在眼睛高度，走到树根底下时那格可能是木头也可能是
/// 空气（树干只有 1 格宽），偶尔会抖一下报"没有"。两次采样都空才认。
const MISS_CONFIRM: u32 = 2;

pub struct Gather {
    pub mode: Mode,
    /// 本次连采已砍几棵（够 `quota` 就去休息）
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
    /// 「树可能砍掉了」的候选状态。
    ///
    /// 不能在砍模式下直接数缺口：木头一消失，门控立刻判 `NoWood` 把模式翻成
    /// `Off`，第二拍就进不了计数分支，永远等不到确认。所以要有一个跨模式的
    /// 候选标记。
    pending_fell: bool,
    fell_streak: u32,
    /// 开始砍这一棵的时刻。
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
    pub wood_dist: Option<f32>,
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
            wood_dist: None,
        }
    }

    pub fn line(&self) -> String {
        format!(
            "gather={} why={} 砍={} 断={}({})",
            self.mode.name(),
            self.why.as_str(),
            self.chopped,
            self.interrupted,
            self.interrupted_by.as_str()
        )
    }

    /// 手动重置：`!gather reset` 清配额和冷却，让它现在立刻可以开工。
    ///
    /// 连 `pending_fell` 一起清 —— 不清的话，如果正好卡在"树没了但还没确认"
    /// 那一拍，reset 之后第一次 tick 会立刻多算一棵，配额莫名其妙少一格。
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

        // 7. 有没有木头。`None` = 视野内没有（数据包 8 方向 × 3 档都没命中）。
        let d = match w.wood {
            None => return (Tri::Neg, Why::NoWood),
            Some(d) => d,
        };
        if d > cfg.max_walk {
            // 看得见但太远：走过去的路上风险不可控，不去
            return (Tri::Neg, Why::TooFar);
        }

        // 8. 配额。砍够了就走，别把整片林子推平 —— 也给自己一个"回城"的节奏。
        //    注意 `quota = 0` 是「不限制」。少了 `> 0` 这个条件，`chopped >= 0`
        //    恒真，等于把采集永久锁死在"配额已完成"。
        if cfg.quota > 0 && self.chopped >= cfg.quota {
            return (Tri::Neg, Why::QuotaDone);
        }

        (Tri::Pos, Why::Ok)
    }

    /// 每拍推进一次。返回值决定行为层这一拍做什么。
    pub fn tick(&mut self, w: &WorldView, cfg: &GatherCfg, now_ms: u64) -> Mode {
        let (g, why) = self.gate(w, cfg, now_ms);
        self.gate = g;
        self.why = why;
        self.wood_dist = w.wood;

        // ---- 记录"砍掉了一棵" ----
        // 判据：上一拍正在砍，这一拍木头没了。缺失要连续两拍才认，
        // 因为判定射线打在眼睛高度，走到树根底下时会偶尔漏一格。
        if self.mode == Mode::Chop && w.wood.is_none() {
            self.pending_fell = true;
        }
        if w.wood.is_some() {
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
        if self.chopped >= cfg.quota && self.rest_until == 0 && cfg.quota > 0 {
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
                let was_working = matches!(self.mode, Mode::Approach | Mode::Chop | Mode::Scan);
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

                let d = w.wood.unwrap_or(f32::INFINITY);

                // 砍太久还没断 → 判断错了。别在那儿杵着，进冷却换个地方。
                if let (Mode::Chop, Some(since)) = (self.mode, self.chop_since) {
                    if now_ms.saturating_sub(since) > cfg.max_chop_ms {
                        self.interrupted += 1;
                        self.cd_until = now_ms + cfg.cooldown_ms;
                        self.chop_since = None;
                        self.why = Why::LostTree;
                        self.interrupted_by = Why::LostTree;
                        self.mode = Mode::Cooldown;
                        return self.mode;
                    }
                }

                if d <= cfg.reach {
                    // 只在「第一次进入砍」时起表。扫视会把 mode 改成 Scan 再改回来，
                    // 如果按 mode 变化重置，插播一次就重新计时，`max_chop_ms` 永远
                    // 触发不了 —— 卡住的树能砍一整天。
                    self.chop_since.get_or_insert(now_ms);
                    self.mode = Mode::Chop;
                } else {
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
        // 判定点在眼睛高度，走到树根下会偶尔报"没有"。只认一次会多算。
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
        // 不确定不是危险：站住等，但不算"被打断"
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
        // 第一次安全不能立刻动手，要先攒够 enter_ms
        let c = cfg();
        let mut g = Gather::new();
        assert_eq!(g.tick(&safe(Some(2.0)), &c, 0), Mode::Wait);
        assert_eq!(g.tick(&safe(Some(2.0)), &c, c.enter_ms - 100), Mode::Wait);
        assert!(g.tick(&safe(Some(2.0)), &c, c.enter_ms + 100).active());
    }

    #[test]
    fn window_resets_when_it_blinks() {
        // 攒到一半又不确定了 → 重头计，不能"断断续续也算连续"
        let c = cfg();
        let mut g = Gather::new();
        g.tick(&safe(Some(2.0)), &c, 0);
        g.tick(&safe(Some(2.0)), &c, 1_000);
        let mut w = safe(Some(2.0));
        w.threat = Tri::Zero;
        g.tick(&w, &c, 1_500);
        // 重新安全，从 2000 开始计
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
        // 威胁判据可能滞后，距离通道独立兜底
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
        // 休息完自动清零重开
        for t in (4_000..12_000).step_by(100) {
            g.tick(&safe(Some(2.0)), &c, t);
        }
        assert_eq!(g.chopped, 0);
        assert!(g.mode.active() || g.mode == Mode::Scan, "{:?}", g.mode);
    }

    #[test]
    fn periodic_scan_interrupts_chopping() {
        // 采集不是无脑按住左键：中间必须停下来抬头看
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
        // 没对准：往需要的方向转
        assert_eq!(aim(Some(0.0), 22.0), Some(Tri::Pos), "要往下看");
        assert_eq!(aim(Some(40.0), 22.0), Some(Tri::Neg), "要往上看");
        // 对准了：一个字都不投，否则会一直转到脚面
        assert_eq!(aim(Some(22.0), 22.0), None);
        assert_eq!(aim(Some(17.0), 22.0), None);
        assert_eq!(aim(Some(27.0), 22.0), None);
        // 没读到角度就别猜
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
        // 回归：`chopped >= quota` 在 quota = 0 时恒真，曾经把采集永久锁死。
        let c = GatherCfg { quota: 0, ..cfg() };
        let mut g = Gather::new();
        g.chopped = 99;
        assert!(warm(&mut g, &safe(Some(2.0)), &c).active());
    }

    #[test]
    fn chop_timer_works_from_time_zero() {
        // 回归：以前用 0 当"还没开始砍"的哨兵，而 now_ms 真的可以是 0。
        // 结果第一拍把 Some(0) 当成 None 重新计时，max_chop_ms 永远不触发。
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
        // 回归：卡在"树没了但还没确认"那一拍时 reset，下一拍会白多算一棵
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
    }
}
