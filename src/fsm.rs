//! 三值决策状态机。
//!
//! 结构分三层，全部以 `Tri` 为载体：
//!
//! 1. **投票（Vote）**：每个「次操作」（前进 / 平移 / 视角 / 攻击 / 跳跃 / 使用 / 换槽）
//!    由若干行为模块独立投票，`Tri::Pos` 想执行、`Tri::Neg` 想反向、`Tri::Zero` 表示
//!    *我这边信息不足*。
//! 2. **裁决（Arbiter）**：按通道汇总。三条硬规则：
//!    - 没人投有效票 → 松手（Zero），不做任何事；
//!    - **未知票权重压过已知票 → 弃权**（这是「不猜」的具体实现）；
//!    - 迟滞（hysteresis）：从 0 转 +1 要过 `threshold`，从 -1 翻到 +1 要过
//!      `threshold + hysteresis`，保证抖动输入不会让角色左右抽搐。
//! 3. **阶段机（Machine）**：Off / Idle / Engage / Retreat / Recover，
//!    转移条件用 K3 运算写，例如「有威胁 且 血够厚」= `threat.and(!low_hp)`，
//!    结果落在 `Zero` 时保守站桩而不是莽。

use crate::trit::Tri;
use std::fmt;

pub const NCH: usize = 7;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Channel {
    /// 前后：Pos=W，Neg=S
    Forward,
    /// 左右：Pos=D，Neg=A
    Strafe,
    /// 水平视角：Pos=右转，Neg=左转
    Yaw,
    /// 垂直视角：Pos=下看，Neg=上看
    Pitch,
    /// 左键
    Attack,
    /// 空格
    Jump,
    /// 右键 / 使用
    Use,
}

impl Channel {
    pub fn index(self) -> usize {
        match self {
            Channel::Forward => 0,
            Channel::Strafe => 1,
            Channel::Yaw => 2,
            Channel::Pitch => 3,
            Channel::Attack => 4,
            Channel::Jump => 5,
            Channel::Use => 6,
        }
    }
    pub fn all() -> [Channel; NCH] {
        [
            Channel::Forward,
            Channel::Strafe,
            Channel::Yaw,
            Channel::Pitch,
            Channel::Attack,
            Channel::Jump,
            Channel::Use,
        ]
    }
    pub fn name(self) -> &'static str {
        match self {
            Channel::Forward => "move",
            Channel::Strafe => "strafe",
            Channel::Yaw => "yaw",
            Channel::Pitch => "pitch",
            Channel::Attack => "attack",
            Channel::Jump => "jump",
            Channel::Use => "use",
        }
    }
    /// (进入阈值, 迟滞量)。视角通道阈值高一点，避免视线抖。
    ///
    /// 注意：迟滞会把「需要翻过去的总分」抬高到 `阈值 + 迟滞`（当上一拍已是
    /// 已知值）。如果某个通道**每拍都有固定权重的票**，而该权重小于这个和，
    /// 这个通道就会永远停在上一拍的值上 —— 表现为「左右换边失效」「视角不转」。
    /// 所以每拍都有人投票的通道（Forward/Strafe/Yaw），迟滞不能比票权重大。
    pub fn policy(self) -> (i32, i32) {
        match self {
            Channel::Forward => (1, 1),
            Channel::Strafe => (1, 1),
            Channel::Yaw => (2, 1),
            Channel::Pitch => (2, 2),
            Channel::Attack => (1, 1),
            Channel::Jump => (1, 2),
            Channel::Use => (2, 2),
        }
    }
}

/// 一次投票。
#[derive(Clone, Copy, Debug)]
pub struct Vote {
    pub channel: Channel,
    pub dir: Tri,
    /// 权重，>0。权重用来表达「这条意见有多重要」，而不是「有多确定」。
    pub weight: i32,
    pub src: &'static str,
    /// 要求**按住**这个通道，而不是点一下。
    ///
    /// 只有 `Attack` 用得上：打怪要脉冲（20 TPS 的帧采样会漏掉太短的按下），
    /// 砍树要一直按住 —— 松手就白砍了。
    pub hold: bool,
}

impl Vote {
    pub fn new(channel: Channel, dir: Tri, weight: i32, src: &'static str) -> Vote {
        Vote {
            channel,
            dir,
            weight: weight.max(0),
            src,
            hold: false,
        }
    }
    /// 按住不放的票。裁决结果仍然是 `Pos`，只是执行层不松手。
    pub fn hold(channel: Channel, weight: i32, src: &'static str) -> Vote {
        Vote {
            channel,
            dir: Tri::Pos,
            weight: weight.max(0),
            src,
            hold: true,
        }
    }
    pub fn abstain(channel: Channel, weight: i32, src: &'static str) -> Vote {
        Vote::new(channel, Tri::Zero, weight, src)
    }
}

/// 感知快照。所有字段都允许未知。
#[derive(Clone, Copy, Debug, Default)]
pub struct WorldView {
    /// 0.0..=1.0，None = 未知
    pub hp: Option<f32>,
    /// 感知到的威胁：Pos 明确有 / Zero 不确定 / Neg 明确没有
    pub threat: Tri,
    /// 最近是否交过手（带衰减）
    pub contact: bool,
    /// 最近敌对生物的距离（格）。None = 没感知到距离，不是“没有敌人”。
    ///
    /// 这个字段是「战术」和「莽」的分界线：不知道距离就只能贴着脸打，
    /// 知道距离才能决定该进、该站、还是该退。
    pub dist: Option<f32>,
    /// 爆炸危险：Pos 必须躲，Neg 明确安全，Zero 边缘/未知。
    pub hazard: Tri,
    /// 附近敌对生物的**数量**。None = 不知道有几只（不是「没有」）。
    ///
    /// 和 `dist` 分开是因为战法要的判据不同：
    /// 「兵贵神速」要单敌（1 只才敢突），「围点打援」要多敌（3 只以上才值得绕侧）。
    /// 只知道最近距离的话，这两种局在数据上长得一模一样。
    pub enemies: Option<u32>,
    /// 饱食度 0..=20。None = 未知。
    ///
    /// 采集要用它：饿着砍树是最亏的 —— 砍完跑不动，而且不能自然回血。
    pub food: Option<i32>,
    /// 最近原木的距离（格）。**None = 视野内没有**，不是「未知」。
    ///
    /// 数据包用 8 个方向 × 3 档距离做射线判定，全部没命中就是 `None`。
    /// 这个字段只在 ±45° 的 8 个方位上有分辨率，够用来"朝那边走"，
    /// 不够用来精确瞄准 —— 走过去之后距离会更新，闭环修正。
    pub wood: Option<f32>,
    /// 木头相对当前朝向的方位：`0` 正前，负左正右，单位 45°（范围 -3..=3）。
    pub wood_dir: Option<i32>,
    /// 最近矿石的距离（格）。**None = 视野内没有**，不是「未知」。
    ///
    /// 和 `wood` 分开而不是合成一个「最近资源」：砍树和挖矿的门控判据不同
    /// （挖矿还要求脚下有支撑、目标不是岩浆），合起来反而要在下游再拆开。
    pub ore: Option<f32>,
    pub ore_dir: Option<i32>,
    /// 八方位墙体掩码，见 [`crate::perceive::wall_at`]。
    ///
    /// `None` = 快通道没开，**不知道有没有墙**。这个区分很重要：把「不知道」
    /// 当成「没有墙」，AI 就会以全速撞上去然后原地推墙 —— 玩家看到的是
    /// 「这 AI 傻了」，而实际原因是它压根没收到过这一维的数据。
    pub wall: Option<u8>,
    /// 准星正对的方块。挖矿挖的就是它。
    pub blk_front: Option<crate::perceive::Block>,
    /// 脚下踩着的方块。挖矿最怕的就是把这块挖了。
    pub blk_below: Option<crate::perceive::Block>,
    /// 正前方是一格高的台阶（能跳过去）吗？`None` = 不知道。
    pub step: Option<bool>,
    /// 实测的感知帧间隔（毫秒，滑动平均）。None = 还没量出来。
    ///
    /// 行为层拿它算「推一下视角之后要等多久」。写死数值两头不讨好：
    /// 按 1 秒等，装了快通道也是白装；按 100ms 等，只有慢通道时一路转过头。
    pub sense_gap_ms: Option<f32>,
    /// 是否夜晚。None = 不知道游戏内时间。
    pub night: Option<bool>,
    /// 视线俯仰角（度，负=下看）。None = 没读到。
    ///
    /// 只给方向的通道 + 这个坐标 = 闭环。砍树时 AI 才能"把眼睛压到树干上"，
    /// 而不是每拍投一次 `Pos` 一路转到脚面。
    pub pitch: Option<f32>,
    /// 是否卡住（位移长时间为 0）
    pub stuck: bool,
    /// 当前坐标（x, y, z）。None = 还没读到过。
    pub pos: Option<(f32, f32, f32)>,
    pub dead: bool,
    /// 人类按下 F8 关掉 AI
    pub human_takeover: bool,
    pub tick: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// AI 关闭，全部松手
    Off,
    /// 无事可做，原地待命
    Idle,
    /// 交战
    Engage,
    /// 撤退
    Retreat,
    /// 死亡 → 点重生
    Recover,
}

impl Phase {
    /// 给人看的中文名。
    ///
    /// 单独一个方法而不是改 [`Phase::name`]：`name()` 是
    /// `Decision::encode` 写进 `trit-agent-run.log` 的段名，
    /// 换成中文会让日志从「一行固定宽度的 ASCII」变成需要按列宽解析的东西。
    pub fn zh(self) -> &'static str {
        match self {
            Phase::Off => "停机",
            Phase::Idle => "待命",
            Phase::Engage => "交战",
            Phase::Retreat => "撤退",
            Phase::Recover => "重生",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Phase::Off => "off",
            Phase::Idle => "idle",
            Phase::Engage => "engage",
            Phase::Retreat => "retreat",
            Phase::Recover => "recover",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub dirs: [Tri; NCH],
    pub phase: Phase,
    /// 需要点一次重生按钮
    pub recover_click: bool,
    /// 左键要**一直按住**（砍树 / 挖矿），而不是点一下。
    ///
    /// 单独一个字段而不是塞进 `dirs`：`dirs` 是三值的"方向"，
    /// "按住"是执行层的时序问题，两者正交 —— 打怪是 Pos + 点一下，
    /// 砍树是 Pos + 按住，方向一样。
    pub attack_hold: bool,
    /// 本帧的阶段判定理由，便于回放排查
    pub reason: Reason,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    Off,
    Dead,
    LowHp,
    Contact,
    Threat,
    NoThreat,
    Unknown,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::Off => "off",
            Reason::Dead => "dead",
            Reason::LowHp => "low_hp",
            Reason::Contact => "contact",
            Reason::Threat => "threat",
            Reason::NoThreat => "no_threat",
            Reason::Unknown => "unknown",
        }
    }
}

impl Decision {
    /// 紧凑编码，用于回放：`engage|+0+000-`
    pub fn encode(&self) -> String {
        let mut s = String::with_capacity(24);
        s.push_str(self.phase.name());
        s.push('|');
        for c in Channel::all() {
            s.push(self.dirs[c.index()].ch());
        }
        s
    }
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.encode(), self.reason.name())
    }
}

/// 单通道裁决器（带迟滞与弃权）。
#[derive(Clone, Copy, Debug, Default)]
struct ChannelCell {
    last: Tri,
}

impl ChannelCell {
    fn decide(&mut self, votes: &[Vote], ch: Channel, gate: bool) -> Tri {
        let mut known_w = 0i32;
        let mut unknown_w = 0i32;
        let mut score = 0i32;
        for v in votes.iter().filter(|v| v.channel == ch) {
            match v.dir {
                Tri::Zero => unknown_w += v.weight,
                d => {
                    known_w += v.weight;
                    score += d.to_i32() * v.weight;
                }
            }
        }
        let out = if !gate || known_w == 0 {
            Tri::Zero
        } else if unknown_w > known_w {
            // 关键规则：信息不足时弃权，绝不把 Unknown 猜成动作
            Tri::Zero
        } else {
            let (thr, hyst) = ch.policy();
            let need = if self.last.is_known() { thr + hyst } else { thr };
            if score >= need {
                Tri::Pos
            } else if score <= -need {
                Tri::Neg
            } else {
                self.last
            }
        };
        self.last = out;
        out
    }
}

pub struct Machine {
    pub phase: Phase,
    pub armed: bool,
    cells: [ChannelCell; NCH],
    pub ticks: u64,
}

impl Default for Machine {
    fn default() -> Self {
        Machine::new()
    }
}

impl Machine {
    pub fn new() -> Machine {
        Machine {
            phase: Phase::Off,
            armed: false,
            cells: [ChannelCell::default(); NCH],
            ticks: 0,
        }
    }

    pub fn set_armed(&mut self, armed: bool) {
        self.armed = armed;
        if !armed {
            self.phase = Phase::Off;
            for c in self.cells.iter_mut() {
                c.last = Tri::Zero;
            }
        }
    }

    /// 是否交战中（供行为模块参考）。
    pub fn engaging(&self) -> bool {
        self.phase == Phase::Engage
    }

    /// 推进一个决策周期，返回本周期每个通道的方向。
    ///
    /// `channels_enabled` 与 `Channel` 一一对应；false 表示该「次操作」被人为关掉，
    /// 该通道强制为 `Zero`（彻底松手）。
    pub fn step(
        &mut self,
        w: &WorldView,
        votes: &[Vote],
        channels_enabled: &[bool; NCH],
    ) -> Decision {
        self.ticks += 1;

        let (phase, reason, recover_click) = self.next_phase(w);

        // 除 Off/Recover 外都允许行动；Recover 只点重生不按键
        let gate = phase == Phase::Idle || phase == Phase::Engage || phase == Phase::Retreat;

        let mut dirs = [Tri::Zero; NCH];
        let mut used_votes: Vec<Vote> = Vec::with_capacity(votes.len());

        for v in votes {
            let idx = v.channel.index();
            if !channels_enabled[idx] {
                continue;
            }
            used_votes.push(*v);
        }

        for ch in Channel::all() {
            dirs[ch.index()] = self.cells[ch.index()].decide(&used_votes, ch, gate);
        }

        // 撤退阶段：强制向后，且不接受前进投票
        if phase == Phase::Retreat {
            dirs[Channel::Forward.index()] = Tri::Neg;
        }
        if phase == Phase::Recover {
            for d in dirs.iter_mut() {
                *d = Tri::Zero;
            }
        }

        self.phase = phase;
        // 「按住」只看**本拍实际投票**，不看裁决后的 dirs。
        // 裁决器有迟滞，本拍没人投 Pos 时 dirs 可能停在上一拍的 Pos 上；
        // 如果照着 dirs 算 hold，扫视期间就会一直按住左键不放。
        let attack_hold = gate
            && dirs[Channel::Attack.index()] == Tri::Pos
            && used_votes
                .iter()
                .any(|v| v.channel == Channel::Attack && v.dir == Tri::Pos && v.hold);
        Decision {
            dirs,
            phase,
            recover_click,
            attack_hold,
            reason,
        }
    }

    /// 只读地预判本周期阶段。行为模块需要「当前」阶段来决定投什么票，
    /// 但阶段又由投票结果决定 —— 所以先 peek 再 step，同一套输入算两遍，
    /// 结果必然一致（纯函数，无副作用），也顺带消掉了行为落后一拍的问题。
    pub fn peek(&self, w: &WorldView) -> Phase {
        self.next_phase(w).0
    }

    fn next_phase(&self, w: &WorldView) -> (Phase, Reason, bool) {
        if w.human_takeover || !self.armed {
            return (Phase::Off, Reason::Off, false);
        }
        if w.dead {
            return (Phase::Recover, Reason::Dead, true);
        }

        // 血量三值化：Neg = 危险，Zero = 临界/未知，Pos = 安全
        let hp = match w.hp {
            Some(h) if h <= LOW_BAND_LO => Tri::Neg,
            Some(h) if h < LOW_BAND_HI => Tri::Zero,
            Some(_) => Tri::Pos,
            None => Tri::Zero,
        };

        // 刚交过手会把「未知威胁」抬成「有威胁」；K3 的 or 正好是这么算的
        let threat = if w.contact {
            w.threat.or(Tri::Pos)
        } else {
            w.threat
        };

        // 转移表本身就是个 3x3 的三值函数。写成表而不是布尔表达式，
        // 是因为 K3 的 `and` 有 Neg 吸收性，直接套会得出「健康 → 不交战」这种错结论。
        match (threat, hp) {
            (Tri::Pos, Tri::Neg) => (Phase::Retreat, Reason::LowHp, false),
            (Tri::Pos, _) => (
                Phase::Engage,
                if w.contact {
                    Reason::Contact
                } else {
                    Reason::Threat
                },
                false,
            ),
            (Tri::Zero, _) => (Phase::Idle, Reason::Unknown, false),
            (Tri::Neg, _) => (Phase::Idle, Reason::NoThreat, false),
        }
    }
}

/// 血量「临界带」：低于 LO 视为危险，LO..HI 视为不确定。
const LOW_BAND_LO: f32 = 0.35;
const LOW_BAND_HI: f32 = 0.55;

#[cfg(test)]
mod tests {
    use super::*;

    fn all_on() -> [bool; NCH] {
        [true; NCH]
    }

    #[test]
    fn off_when_disarmed() {
        let mut m = Machine::new();
        let w = WorldView::default();
        let d = m.step(&w, &[], &all_on());
        assert_eq!(d.phase, Phase::Off);
        assert!(d.dirs.iter().all(|t| *t == Tri::Zero));
    }

    #[test]
    fn unknown_dominates_abstains() {
        let mut m = Machine::new();
        m.set_armed(true);
        let w = WorldView {
            hp: Some(1.0),
            threat: Tri::Neg,
            ..Default::default()
        };
        // 3 权重未知 vs 1 权重想攻击 → 弃权
        let votes = [
            Vote::new(Channel::Attack, Tri::Pos, 1, "combat"),
            Vote::abstain(Channel::Attack, 3, "scanner"),
        ];
        let d = m.step(&w, &votes, &all_on());
        assert_eq!(d.dirs[Channel::Attack.index()], Tri::Zero);
    }

    #[test]
    fn hysteresis_blocks_flip_flop() {
        let mut m = Machine::new();
        m.set_armed(true);
        let w = WorldView {
            hp: Some(1.0),
            threat: Tri::Neg,
            ..Default::default()
        };
        // 先稳定拿到 Pos
        let pos = [Vote::new(Channel::Forward, Tri::Pos, 2, "patrol")];
        let d = m.step(&w, &pos, &all_on());
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Pos);

        // 反向只有 1 权重，达不到 threshold(1)+hysteresis(1)，保持 Pos
        let weak_neg = [Vote::new(Channel::Forward, Tri::Neg, 1, "noise")];
        let d = m.step(&w, &weak_neg, &all_on());
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Pos);

        // 强反对才翻
        let strong_neg = [Vote::new(Channel::Forward, Tri::Neg, 2, "flee")];
        let d = m.step(&w, &strong_neg, &all_on());
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Neg);
    }

    #[test]
    fn disabled_channel_is_silent() {
        let mut m = Machine::new();
        m.set_armed(true);
        let w = WorldView {
            hp: Some(1.0),
            ..Default::default()
        };
        let mut ch = all_on();
        ch[Channel::Attack.index()] = false;
        let votes = [Vote::new(Channel::Attack, Tri::Pos, 5, "combat")];
        let d = m.step(&w, &votes, &ch);
        assert_eq!(d.dirs[Channel::Attack.index()], Tri::Zero);
    }

    #[test]
    fn low_hp_with_threat_retreats() {
        let mut m = Machine::new();
        m.set_armed(true);
        let w = WorldView {
            hp: Some(0.2),
            threat: Tri::Pos,
            ..Default::default()
        };
        let d = m.step(&w, &[], &all_on());
        assert_eq!(d.phase, Phase::Retreat);
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Neg);
    }

    #[test]
    fn dead_triggers_recover() {
        let mut m = Machine::new();
        m.set_armed(true);
        let w = WorldView {
            dead: true,
            ..Default::default()
        };
        let d = m.step(&w, &[], &all_on());
        assert_eq!(d.phase, Phase::Recover);
        assert!(d.recover_click);
    }

    #[test]
    fn unknown_threat_does_not_engage() {
        let mut m = Machine::new();
        m.set_armed(true);
        let w = WorldView {
            hp: Some(1.0),
            threat: Tri::Zero,
            contact: false,
            ..Default::default()
        };
        let d = m.step(&w, &[], &all_on());
        assert_eq!(d.phase, Phase::Idle);
        assert_eq!(d.reason, Reason::Unknown);
    }
}
