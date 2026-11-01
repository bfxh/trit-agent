//! 战法层：把《孙子兵法》里**能落到键鼠动作上**的那几条，翻译成投票修正。
//!
//! 三条设计约束，都是踩过坑才定下来的：
//!
//! 1. **不新增行为**。行为负责「投什么票」，战法只负责「加权 / 压票」。
//!    早先试过把战术直接写成行为，结果和移植过来的 Goal（Ranging/Guard/Breach）
//!    各说各话：一边要冲，一边要站，加权后角色原地抽搐。
//! 2. **只追加票，不改别人的票**。裁决器是加权求和 + 未知吸收
//!    （见 [`crate::fsm::ChannelCell::decide`]），追加一条高权重弃权票就能压住
//!    前进，不需要去改源票的字段，也就不会破坏别处的假设。
//! 3. **一次只生效一条**。战法之间互斥，优先级见 [`select`]。
//!    两条同时生效必然互相抵消 —— 一条让你冲、一条让你停。
//!
//! 每一条都是「判据 + 动作」的纯函数，没有隐藏状态，所以能单测。

use crate::fsm::{Channel, Vote, WorldView};
use crate::trit::Tri;

/// 五条战法。名字按《孙子兵法》／三十六计取，括注是它在 Minecraft 里的实际含义。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strat {
    /// 走为上：血薄且被围，先脱离再谈别的。
    Retreat,
    /// 以逸待劳：血不满、敌人还远，那就站着回血等它走过来。
    Wait,
    /// 兵贵神速：血厚、只有一只、距离够近 —— 这时候犹豫才是错的。
    Blitz,
    /// 围点打援：多只的时候不冲进人堆，停在边缘把最前面那只放出来单杀。
    Ambush,
    /// 声东击西：交战中持续侧向移动，永远不在原地挨打。
    Feint,
}

impl Strat {
    pub fn id(self) -> &'static str {
        match self {
            Strat::Retreat => "retreat",
            Strat::Wait => "wait",
            Strat::Blitz => "blitz",
            Strat::Ambush => "ambush",
            Strat::Feint => "feint",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Strat::Retreat => "走为上",
            Strat::Wait => "以逸待劳",
            Strat::Blitz => "兵贵神速",
            Strat::Ambush => "围点打援",
            Strat::Feint => "声东击西",
        }
    }
    pub fn source(self) -> &'static str {
        match self {
            Strat::Retreat => "三十六计·走为上",
            Strat::Wait => "孙子兵法·军争篇",
            Strat::Blitz => "孙子兵法·九地篇",
            Strat::Ambush => "孙子兵法·虚实篇",
            Strat::Feint => "三十六计·声东击西",
        }
    }
    pub fn all() -> [Strat; 5] {
        [
            Strat::Retreat,
            Strat::Wait,
            Strat::Blitz,
            Strat::Ambush,
            Strat::Feint,
        ]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Strategies {
    pub retreat: bool,
    pub wait: bool,
    pub blitz: bool,
    pub ambush: bool,
    pub feint: bool,
    /// 声东击西的换向周期
    pub feint_ms: u64,
}

impl Default for Strategies {
    fn default() -> Self {
        Strategies {
            retreat: true,
            wait: true,
            blitz: true,
            ambush: true,
            feint: true,
            feint_ms: 700,
        }
    }
}

impl Strategies {
    pub fn enabled(&self, s: Strat) -> bool {
        match s {
            Strat::Retreat => self.retreat,
            Strat::Wait => self.wait,
            Strat::Blitz => self.blitz,
            Strat::Ambush => self.ambush,
            Strat::Feint => self.feint,
        }
    }
}

/// 选一条战法。**第一个命中的生效**，所以顺序就是优先级。
///
/// 顺序的道理：
/// - 保命永远第一（走为上）；
/// - 然后是「别白挨打」（以逸待劳：血不满就别硬冲）；
/// - 再是「能一波就一波」（兵贵神速：单敌且血厚）；
/// - 然后是「别冲进人堆」（围点打援）；
/// - 最后是「贴身时别站着」（声东击西）。
///
/// 血量和敌人数**未知时一律不启用**对应战法 —— 战法是有代价的激进决策，
/// 拿未知当已知去激进，正是这套系统最不该做的事。
pub fn select(w: &WorldView, s: &Strategies) -> Option<Strat> {
    if w.dead || w.human_takeover || w.threat != Tri::Pos {
        return None;
    }
    let hp = w.hp;
    let d = w.dist;
    let n = w.enemies;

    if s.retreat {
        if let Some(hp) = hp {
            let outnumbered = n.map(|n| n >= 2).unwrap_or(false);
            // 血量极薄，或者血不满又被人围 —— 都是先跑再说
            if hp < 0.35 || (hp < 0.5 && outnumbered) {
                return Some(Strat::Retreat);
            }
        }
    }

    if s.wait {
        if let (Some(hp), Some(d)) = (hp, d) {
            // 血不满 + 敌人还在 10 格外：走过去只会以残血接战
            if hp < 0.75 && d > 10.0 {
                return Some(Strat::Wait);
            }
        }
    }

    if s.blitz {
        if let (Some(hp), Some(d), Some(n)) = (hp, d, n) {
            // 只在「还在接近的路上」加速。已经进入 6 格交战带就交回距离带，
            // 否则会和 Ranging 的「位置刚好、站住」直接对冲 —— 一边让冲一边让站，
            // 表现是角色在 4~5 格处反复抖动。第一版写 d<=12 就是这个后果。
            if hp > 0.8 && n == 1 && d > 6.0 && d <= 12.0 {
                return Some(Strat::Blitz);
            }
        }
    }

    if s.ambush {
        if let (Some(d), Some(n)) = (d, n) {
            if n >= 2 && d >= 6.0 {
                return Some(Strat::Ambush);
            }
        }
    }

    if s.feint {
        if w.contact {
            if let Some(d) = d {
                if d <= 6.0 {
                    return Some(Strat::Feint);
                }
            }
        }
    }

    None
}

/// 战法层自己的状态：侧移方向和换向计时。
#[derive(Default)]
pub struct StratState {
    pub active: Option<Strat>,
    pub side: Option<Tri>,
    next_flip: u64,
    /// 上一次生效的战法 id，变化时记日志用
    pub last_logged: &'static str,
}

impl StratState {
    pub fn new() -> StratState {
        StratState {
            active: None,
            side: Some(Tri::Pos),
            next_flip: 0,
            last_logged: "",
        }
    }
}

/// 把战法翻译成追加票。
pub fn apply(active: Option<Strat>, votes: &mut Vec<Vote>, now_ms: u64, st: &mut StratState, s: &Strategies) {
    st.active = active;
    let Some(strat) = active else { return };
    let side = st.side.unwrap_or(Tri::Pos);
    match strat {
        // 走为上：后退 + 斜向脱离 + 跳（跳能翻过一格高的坎）
        Strat::Retreat => {
            votes.push(Vote::new(Channel::Forward, Tri::Neg, 6, "strat_retreat"));
            votes.push(Vote::new(Channel::Strafe, side, 4, "strat_retreat"));
            votes.push(Vote::new(Channel::Jump, Tri::Pos, 2, "strat_retreat"));
        }
        // 以逸待劳：高权重弃权把前进压掉 —— 站着比走过去强
        Strat::Wait => {
            votes.push(Vote::abstain(Channel::Forward, 6, "strat_wait"));
            votes.push(Vote::abstain(Channel::Strafe, 3, "strat_wait"));
        }
        // 兵贵神速：把「距离带站住」的票压过去，硬冲
        Strat::Blitz => {
            votes.push(Vote::new(Channel::Forward, Tri::Pos, 6, "strat_blitz"));
            votes.push(Vote::new(Channel::Attack, Tri::Pos, 3, "strat_blitz"));
        }
        // 围点打援：不冲群（弃权压前进），绕到侧面把最前面那只放出来
        Strat::Ambush => {
            votes.push(Vote::abstain(Channel::Forward, 4, "strat_ambush"));
            votes.push(Vote::new(Channel::Strafe, side, 5, "strat_ambush"));
            if now_ms >= st.next_flip {
                st.side = Some(side.not());
                st.next_flip = now_ms + s.feint_ms;
            }
        }
        // 声东击西：持续侧移，方向周期性翻转
        Strat::Feint => {
            if now_ms >= st.next_flip {
                st.side = Some(side.not());
                st.next_flip = now_ms + s.feint_ms;
            }
            votes.push(Vote::new(
                Channel::Strafe,
                st.side.unwrap_or(Tri::Pos),
                5,
                "strat_feint",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(hp: f32, d: f32, n: u32) -> WorldView {
        WorldView {
            hp: Some(hp),
            threat: Tri::Pos,
            dist: Some(d),
            enemies: Some(n),
            contact: true,
            ..Default::default()
        }
    }

    fn all() -> Strategies {
        Strategies::default()
    }

    #[test]
    fn low_hp_retreats_first() {
        // 血薄 + 被围 → 走为上，哪怕对面只有 3 格（贴身也得先脱）
        assert_eq!(select(&w(0.2, 3.0, 3), &all()), Some(Strat::Retreat));
    }

    #[test]
    fn midway_hp_outnumbered_retreats() {
        assert_eq!(select(&w(0.45, 5.0, 2), &all()), Some(Strat::Retreat));
        // 同样血量但只有一只 → 不退，交给后面的战法判
        assert_ne!(select(&w(0.45, 5.0, 1), &all()), Some(Strat::Retreat));
    }

    #[test]
    fn wait_beats_blitz_when_hp_is_low() {
        // 血不满 + 敌人在 10 格外 → 站着等，而不是冲过去残血接战
        assert_eq!(select(&w(0.6, 20.0, 1), &all()), Some(Strat::Wait));
        // 血满 + 单敌 + 12 格内 → 才轮到兵贵神速
        assert_eq!(select(&w(1.0, 10.0, 1), &all()), Some(Strat::Blitz));
    }

    #[test]
    fn blitz_never_overrides_the_engage_band() {
        // 回归：第一版写 d<=12，结果在 4.5 格也判「兵贵神速」，
        // 把 Ranging 的「站住」压掉，角色一路怼到脸上。
        assert_ne!(select(&w(1.0, 4.5, 1), &all()), Some(Strat::Blitz));
        assert_ne!(select(&w(1.0, 3.0, 1), &all()), Some(Strat::Blitz));
        // 接近途中（6~12 格）才是它的战场
        assert_eq!(select(&w(1.0, 10.0, 1), &all()), Some(Strat::Blitz));
    }

    #[test]
    fn ambush_only_when_outnumbered_mid_range() {
        assert_eq!(select(&w(1.0, 8.0, 3), &all()), Some(Strat::Ambush));
        // 贴到 4 格就不围了，该打就打
        assert_eq!(select(&w(1.0, 4.0, 3), &all()), Some(Strat::Feint));
    }

    #[test]
    fn unknown_data_disables_aggressive_strategies() {
        // 血量未知 → 不许判「血厚可以冲」，也不许判「血薄快跑」
        let mut x = w(1.0, 10.0, 1);
        x.hp = None;
        assert_eq!(select(&x, &all()), None);

        // 敌人数未知 → 兵贵神速/围点打援都判不了，只剩声东击西（只看接触距离）
        let mut y = w(1.0, 4.0, 1);
        y.enemies = None;
        assert_eq!(select(&y, &all()), Some(Strat::Feint));
    }

    #[test]
    fn human_takeover_and_dead_disable_everything() {
        let mut x = w(0.2, 3.0, 3);
        x.human_takeover = true;
        assert_eq!(select(&x, &all()), None);
        let mut y = w(0.2, 3.0, 3);
        y.dead = true;
        assert_eq!(select(&y, &all()), None);
    }

    #[test]
    fn each_switch_can_turn_it_off() {
        let mut s = all();
        s.retreat = false;
        assert_ne!(select(&w(0.2, 3.0, 3), &s), Some(Strat::Retreat));
        let mut s2 = all();
        s2.blitz = false;
        assert_ne!(select(&w(1.0, 10.0, 1), &s2), Some(Strat::Blitz));
    }

    #[test]
    fn wait_pushes_zero_weight_past_forward_votes() {
        // 以逸待劳必须真的能压住「巡逻前进」那种票权 3 的票
        let mut votes = vec![Vote::new(Channel::Forward, Tri::Pos, 3, "patrol")];
        let mut st = StratState::new();
        apply(Some(Strat::Wait), &mut votes, 0, &mut st, &all());
        let unknown: i32 = votes
            .iter()
            .filter(|v| v.channel == Channel::Forward && v.dir == Tri::Zero)
            .map(|v| v.weight)
            .sum();
        let known: i32 = votes
            .iter()
            .filter(|v| v.channel == Channel::Forward && v.dir != Tri::Zero)
            .map(|v| v.weight)
            .sum();
        assert!(unknown > known, "弃权票必须压过前进票，否则人还在往前走");
    }

    #[test]
    fn feint_flips_side_periodically() {
        let mut votes = Vec::new();
        let mut st = StratState::new();
        let s = all();
        apply(Some(Strat::Feint), &mut votes, 0, &mut st, &s);
        let first = st.side.unwrap();
        apply(Some(Strat::Feint), &mut votes, s.feint_ms + 1, &mut st, &s);
        assert_ne!(st.side.unwrap(), first, "到点必须换向");
    }

    #[test]
    fn no_strategy_appends_nothing() {
        let mut votes = vec![Vote::new(Channel::Forward, Tri::Pos, 3, "patrol")];
        let mut st = StratState::new();
        apply(None, &mut votes, 0, &mut st, &all());
        assert_eq!(votes.len(), 1, "没有战法就不该多出任何票");
        assert_eq!(st.active, None);
    }
}
