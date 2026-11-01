//! 共同意识：**一个决策核心，多个身体**。
//!
//! 这里不是「每只宠物配一个小 AI」—— 那是多智能体，会互相抢目标、抢路、
//! 把怪拉成一团。这里只有一个意识：同一份 [`WorldView`]、同一套战法判据，
//! 同时决定「玩家自己怎么打」和「傀儡／狼去打谁」。
//!
//! 三种身体的能力天差地别，所以执行层是分开的：
//!
//! | 身体 | 执行手段 | 能做的事 |
//! |---|---|---|
//! | 玩家角色 | 键鼠注入（`SendInput`） | 全部，逐帧 |
//! | 铁傀儡／狼 | 游戏指令 → 数据包 function | 意图级：打谁、集合、撤回 |
//! | 未来（对讲机等） | 实现 [`CommandSink`] | 见 `docs/CONTROLS.md` |
//!
//! 为什么从属实体只能「意图级」：它们的 AI 是原版写死的 Goal，
//! 没有可注入的输入通道。唯一能真正改变它们行为的是 `AngryAt`／`AngerTime`
//! 这两个 NBT 字段（而且只有 wolf / iron_golem / polar_bear 的 Goal 会读它，
//! 见 `crate::npc` 里的考证）。所以「操控宠物」在 1.18.2 的物理上限就是：
//! **给它指定目标、让它集合、让它撤**，中间怎么走、怎么挥爪子管不了。
//!
//! 把这层说明白，比假装能逐帧操控它们要诚实。

use crate::fsm::WorldView;
use crate::trit::Tri;

/// 从属实体的种类。数据包用同名 tag 圈定（`trit_golem` / `trit_hound`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// 铁傀儡 / 雪傀儡：肉盾，扛伤害
    Golem,
    /// 狼 / 猫：机动，追残血
    Hound,
}

impl Role {
    pub fn tag(self) -> &'static str {
        match self {
            Role::Golem => "trit_golem",
            Role::Hound => "trit_hound",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Role::Golem => "傀儡",
            Role::Hound => "狼",
        }
    }
}

/// 意识下发给身体的意图。只有四种 —— 多了数据包侧也表达不了。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    /// 待命：原地不动，不主动开战
    Hold,
    /// 交战：锁定最近的敌对生物
    Engage,
    /// 集合：走到玩家身边（一起压上）
    Regroup,
    /// 召回：紧急脱离战斗，回来护主
    Recall,
}

impl Intent {
    /// 给人看的中文名。给 `!squad` 回显和状态行用；
    /// 真正下发给数据包的是 [`Intent::command`]，两边互不影响。
    pub fn name(self) -> &'static str {
        match self {
            Intent::Hold => "待命",
            Intent::Engage => "交战",
            Intent::Regroup => "集合",
            Intent::Recall => "召回",
        }
    }
    pub fn all() -> [Intent; 4] {
        [Intent::Hold, Intent::Engage, Intent::Regroup, Intent::Recall]
    }
    /// 对应的数据包 function。完整写法带前导 `/`，因为要走命令框。
    pub fn command(self) -> &'static str {
        match self {
            Intent::Hold => "/function squad:hold",
            Intent::Engage => "/function squad:engage",
            Intent::Regroup => "/function squad:regroup",
            Intent::Recall => "/function squad:recall",
        }
    }
    pub fn parse(s: &str) -> Option<Intent> {
        match s.trim().to_ascii_lowercase().as_str() {
            "hold" | "待命" => Some(Intent::Hold),
            "engage" | "打" | "交战" => Some(Intent::Engage),
            "regroup" | "集合" => Some(Intent::Regroup),
            "recall" | "召回" | "撤" => Some(Intent::Recall),
            _ => None,
        }
    }
}

/// 指令通道。**这是给别人扩展留的唯一接口**。
///
/// 想接对讲机、语音、第二屏、HTTP，就实现这个 trait，然后在
/// `docs/CONTROLS.md` 里登记一行。上层（意识、战法）完全不用改。
pub trait CommandSink {
    fn kind(&self) -> &'static str;
    /// 发一条指令。返回 false 表示**没发出去**（不在前台、通道断开等），
    /// 上层据此决定要不要下一拍重试。
    fn send(&mut self, cmd: &str) -> bool;
}

/// 只记录不发。用来在没有游戏时跑通整条逻辑，也是 CI 的默认通道。
#[derive(Default, Debug)]
pub struct DrySink {
    pub log: Vec<String>,
}

impl CommandSink for DrySink {
    fn kind(&self) -> &'static str {
        "dry"
    }
    fn send(&mut self, cmd: &str) -> bool {
        self.log.push(cmd.to_string());
        true
    }
}

/// 共同意识的配置快照。从 `Config` 拷一份，避免到处借 `&Config`。
#[derive(Clone, Copy, Debug)]
pub struct SquadCfg {
    pub enabled: bool,
    /// 决定「压上」的距离（格）。超过就先集合，不要各打各的
    pub engage_dist: f32,
    /// 低于这个血量且有威胁 → 召回护主
    pub recall_hp: f32,
    /// 两条指令之间的最小间隔。指令要打字，太密会自己打断自己
    pub min_interval_ms: u64,
}

impl Default for SquadCfg {
    fn default() -> Self {
        SquadCfg {
            enabled: false,
            engage_dist: 12.0,
            recall_hp: 0.45,
            min_interval_ms: 1200,
        }
    }
}

/// 意图判据。纯函数，方便单测。
///
/// 和战法层用同一份 `WorldView`，这就是「共同意识」的字面意思：
/// 傀儡看到的威胁和玩家看到的威胁是同一个威胁。
pub fn choose_intent(w: &WorldView, cfg: &SquadCfg) -> Intent {
    // 死了、或者人已经接管 —— 别再指挥它们了，它们会自己打
    if w.dead || w.human_takeover {
        return Intent::Hold;
    }
    if w.threat != Tri::Pos {
        return Intent::Hold;
    }
    // 血薄且有人贴脸：喊回来挡一下。注意这是「召回」不是「撤退」——
    // 玩家的走位由战法层决定，宠物回来是给玩家争取脱身时间。
    if let Some(hp) = w.hp {
        if hp < cfg.recall_hp {
            return Intent::Recall;
        }
    }
    match w.dist {
        Some(d) if d <= cfg.engage_dist => Intent::Engage,
        // 有威胁但还远：先集合，一起压上。各打各的会被逐个击破。
        Some(_) => Intent::Regroup,
        // 距离未知：退回保守动作，别让它们盲冲
        None => Intent::Hold,
    }
}

pub struct Squad {
    pub cfg: SquadCfg,
    pub intent: Intent,
    last_sent_ms: u64,
    last_sent: Intent,
    /// 是否已经尝试过发送（成功和失败都算）。
    /// 不能用 `last_sent_ms > 0` 当哨兵 —— now_ms 真的可能是 0，会撞车。
    attempted: bool,
    pub sent: u64,
    /// 想发但没发出去（冷却中 / 通道不肯收）
    pub skipped: u64,
    /// 人工强制指定的意图。Some 时压过自动判据，直到 `!squad auto`
    pub manual: Option<Intent>,
}

impl Squad {
    pub fn new(cfg: SquadCfg) -> Squad {
        Squad {
            cfg,
            intent: Intent::Hold,
            last_sent_ms: 0,
            last_sent: Intent::Hold,
            attempted: false,
            sent: 0,
            skipped: 0,
            manual: None,
        }
    }

    pub fn set_manual(&mut self, i: Option<Intent>) {
        self.manual = i;
    }

    /// 算这一拍该下什么意图（纯判据，不发指令）。
    pub fn plan(&mut self, w: &WorldView) -> Intent {
        self.intent = if !self.cfg.enabled {
            Intent::Hold
        } else {
            self.manual.unwrap_or_else(|| choose_intent(w, &self.cfg))
        };
        self.intent
    }

    /// 每拍调一次：先算意图，需要时经 `sink` 下发。
    /// 返回「这一拍有没有真的发出指令」。
    pub fn tick(&mut self, w: &WorldView, now_ms: u64, sink: &mut dyn CommandSink) -> bool {
        let want = self.plan(w);
        if !self.cfg.enabled {
            return false;
        }
        if want == self.last_sent {
            return false;
        }
        // 冷却：指令是打字打进去的，两条指令挨太近会互相打断
        if self.attempted && now_ms.saturating_sub(self.last_sent_ms) < self.cfg.min_interval_ms {
            self.skipped += 1;
            return false;
        }
        self.attempted = true;
        if sink.send(want.command()) {
            self.sent += 1;
            self.last_sent = want;
            self.last_sent_ms = now_ms;
            true
        } else {
            // 通道没接（游戏不在前台等）。不要更新 last_sent，下一拍还会重试 ——
            // 但**必须**把冷却起点挪过来。否则通道持续失败时每拍都会重试一次，
            // 而一次 type_command 要阻塞 300ms，主循环直接被拖垮。
            self.skipped += 1;
            self.last_sent_ms = now_ms;
            false
        }
    }

    /// 不经过冷却，立刻把当前意图重发一遍。用于 `!squad resend`。
    pub fn force_resend(&mut self, w: &WorldView, now_ms: u64, sink: &mut dyn CommandSink) -> bool {
        if !self.cfg.enabled {
            return false;
        }
        let want = self.plan(w);
        self.attempted = true;
        if sink.send(want.command()) {
            self.sent += 1;
            self.last_sent = want;
            self.last_sent_ms = now_ms;
            true
        } else {
            self.skipped += 1;
            false
        }
    }

    pub fn line(&self, channel: &str) -> String {
        format!(
            "小队={} 通道={} 意图={}{} 已发={} 跳过={}",
            if self.cfg.enabled { "开" } else { "关" },
            channel,
            self.intent.name(),
            match self.manual {
                Some(_) => "(手动)",
                None => "",
            },
            self.sent,
            self.skipped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(hp: f32, d: Option<f32>) -> WorldView {
        WorldView {
            hp: Some(hp),
            threat: Tri::Pos,
            dist: d,
            contact: true,
            ..Default::default()
        }
    }

    fn on() -> SquadCfg {
        SquadCfg {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn engages_when_close_and_healthy() {
        assert_eq!(choose_intent(&w(1.0, Some(5.0)), &on()), Intent::Engage);
    }

    #[test]
    fn regroups_when_enemy_is_far() {
        assert_eq!(choose_intent(&w(1.0, Some(30.0)), &on()), Intent::Regroup);
    }

    #[test]
    fn recalls_when_hp_is_low() {
        assert_eq!(choose_intent(&w(0.3, Some(5.0)), &on()), Intent::Recall);
    }

    #[test]
    fn unknown_distance_holds_instead_of_charging() {
        // 距离未知就不许让宠物盲冲 —— 这是三值「不猜」在共同意识里的落地
        assert_eq!(choose_intent(&w(1.0, None), &on()), Intent::Hold);
    }

    #[test]
    fn no_threat_means_hold() {
        let mut x = w(1.0, Some(5.0));
        x.threat = Tri::Neg;
        assert_eq!(choose_intent(&x, &on()), Intent::Hold);
    }

    #[test]
    fn human_takeover_parks_everyone() {
        let mut x = w(1.0, Some(5.0));
        x.human_takeover = true;
        assert_eq!(choose_intent(&x, &on()), Intent::Hold);
    }

    #[test]
    fn sends_only_on_change_and_respects_cooldown() {
        let mut s = Squad::new(on());
        let mut sink = DrySink::default();
        let close = w(1.0, Some(5.0));
        assert!(s.tick(&close, 0, &mut sink), "第一条指令该发");
        assert!(!s.tick(&close, 5, &mut sink), "意图没变就不该重复发");
        // 变成 regroup，但还在冷却里 → 不发
        assert!(!s.tick(&w(1.0, Some(30.0)), 10, &mut sink));
        assert!(s.skipped > 0);
        // 冷却过了才发
        assert!(s.tick(&w(1.0, Some(30.0)), 5_000, &mut sink));
        assert_eq!(s.sent, 2);
        assert_eq!(sink.log.len(), 2);
        assert_eq!(sink.log[1], "/function squad:regroup");
    }

    #[test]
    fn disabled_squad_never_sends() {
        let mut s = Squad::new(SquadCfg {
            enabled: false,
            ..Default::default()
        });
        let mut sink = DrySink::default();
        assert!(!s.tick(&w(1.0, Some(5.0)), 0, &mut sink));
        assert!(sink.log.is_empty());
        assert_eq!(s.intent, Intent::Hold);
    }

    #[test]
    fn failed_send_is_retried_but_not_in_a_tight_loop() {
        struct Dead;
        impl CommandSink for Dead {
            fn kind(&self) -> &'static str {
                "dead"
            }
            fn send(&mut self, _: &str) -> bool {
                false
            }
        }
        let mut s = Squad::new(on());
        let mut dead = Dead;
        assert!(!s.tick(&w(1.0, Some(5.0)), 0, &mut dead));
        // 没发成功就不能记账成「已发送」，否则这个话题再也发不出去
        assert_eq!(s.sent, 0);
        // 但冷却起点要挪：否则失败时每拍都重试，而一次打字要 300ms
        assert!(!s.tick(&w(1.0, Some(5.0)), 100, &mut dead));
        assert_eq!(s.skipped, 2, "100ms 后的那拍应该被冷却挡住，而不是再试一次");
        // 冷却过了再试
        assert!(!s.tick(&w(1.0, Some(5.0)), 5_000, &mut dead));
        assert_eq!(s.sent, 0);
    }

    #[test]
    fn manual_intent_overrides_sensor() {
        let mut s = Squad::new(on());
        let mut sink = DrySink::default();
        s.set_manual(Some(Intent::Recall));
        assert!(s.tick(&w(1.0, Some(5.0)), 0, &mut sink));
        assert_eq!(s.intent, Intent::Recall);
        assert_eq!(sink.log[0], "/function squad:recall");
    }

    #[test]
    fn command_strings_match_datapack_functions() {
        // 改指令文本时这条会先炸，省得进了游戏才发现 function 不存在
        for i in Intent::all() {
            assert!(i.command().starts_with("/function squad:"));
        }
    }
}
