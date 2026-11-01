//! 把感知、状态机、输出三层接起来，跑「高频节拍」主循环。

use crate::act::{Button, Input};
use crate::behaviors::{Behaviors, Ctx};
use crate::config::{Config, ControlChannel, Mode};
use crate::fsm::{Channel, Decision, Machine, Phase, Vote, WorldView};
use crate::perceive::logtail::LogTail;
use crate::perceive::{LogEvent, Sensor};
use crate::squad::{CommandSink, DrySink, Squad};
use crate::strategy::StratState;
use crate::trit::Tri;
use std::io::Write;
use std::time::{Duration, Instant};

/// 把 [`CommandSink`] 接到键鼠注入上：指令走游戏命令框真发。
struct InputSink<'a>(&'a mut dyn Input);

impl CommandSink for InputSink<'_> {
    fn kind(&self) -> &'static str {
        "chat"
    }
    fn send(&mut self, cmd: &str) -> bool {
        self.0.type_command(cmd)
    }
}

/// 一次点击最少按住多久（毫秒）。16ms 一帧，70ms 保证跨 3~4 帧。
const MIN_CLICK_MS: u64 = 70;
/// 跳跃最少按住多久。
const MIN_JUMP_MS: u64 = 90;
/// 连续点重生超过这么多次就认为点不到，主动交还控制权。
const MAX_RESPAWN_CLICKS: u32 = 12;
/// 卡住判定的滑动窗口。
const STUCK_WINDOW_MS: u64 = 900;
/// 窗口内水平位移低于这个值就算卡住（格）。
const STUCK_MIN_TRAVEL: f32 = 0.4;

/// 需要按几次 F5 才能从 `cur` 档走到 `target` 档。
///
/// 游戏里 F5 是个三态环：第一 → 第三背视 → 第三前视 → 第一。
/// `known == false` 表示「不知道现在在哪一档」，此时按满一圈（3 次必回原点）
/// 再加上目标偏移 —— 用两次多余的按键换一个确定的落点。
pub fn perspective_steps(cur: u8, target: u8, known: bool) -> u8 {
    let target = target % 3;
    if known {
        (target + 3 - (cur % 3)) % 3
    } else {
        3 + target
    }
}

pub struct RunStats {
    pub ticks: u64,
    pub elapsed: Duration,
    pub achieved_hz: f64,
    pub attacks: u64,
    pub decisions: u64,
    pub phase_hist: [u64; 5],
    pub exits: u32,
    /// 因为游戏窗口不在前台而暂停的次数
    pub bg_pauses: u64,
}

pub struct Agent {
    pub cfg: Config,
    pub machine: Machine,
    pub behaviors: Behaviors,
    pub input: Box<dyn Input>,
    pub tail: Option<LogTail>,
    pub sensor: Option<Sensor>,
    /// 最后一次收到传感器数据的时间。用来判断「这条消息有多旧」。
    pub sensor_at: u64,

    /* 感知状态 */
    pub hp: Option<f32>,
    pub threat: Tri,
    pub threat_until: u64,
    pub contact_until: u64,
    pub dead: bool,
    /* 态势 */
    /// 最近敌对生物距离档（0 = 无）
    dist_tier: i32,
    /// 最近爆炸源距离档（0 = 无）
    hazard_tier: i32,
    /// 附近敌对生物数量。None = 还没收到过传感器
    hostiles: Option<u32>,

    /* 战法与共同意识 */
    pub strat: StratState,
    pub squad: Squad,
    /// 采集门控（砍树）。判据全在 [`crate::gather`]，这里只持有状态。
    pub gather: crate::gather::Gather,
    /// `squad_control = dry` 时的落地通道
    dry_cmd: DrySink,
    /* 卡住检测：累计路程窗口 */
    pos_last: Option<(f32, f32, f32)>,
    pos_win_start: u64,
    /// 窗口内累计走过的路程（不是首尾直线距离）
    pos_travel: f32,
    stuck: bool,

    /* 输出状态去重 */
    key_w: bool,
    key_s: bool,
    key_a: bool,
    key_d: bool,
    key_space: bool,
    key_shift: bool,
    btn_left: bool,
    btn_right: bool,
    // 一次按下至少要持续这么久才松开。游戏输入按帧采样（~16ms），
    // 5ms 的按键脉冲有很大概率整帧漏掉，表现就是「AI 不攻击」。
    btn_left_until: u64,
    /// 上一拍决策是不是要求「按住」左键（砍树）。用来区分松手时机：
    /// 脉冲靠超时松，按住靠决策撤回松。
    btn_left_hold: bool,
    btn_right_until: u64,
    space_until: u64,
    respawn_clicks: u32,

    /* 连续量做累加，避免每帧取整丢精度 */
    yaw_px: f32,
    pitch_px: f32,

    next_recover_ms: u64,
    next_log_ms: u64,
    armed: bool,
    channels: [bool; 7],
    run_log: Option<std::fs::File>,
    log_tick_stride: u64,

    /* 视角记账 */
    /// 0 = 第一视角，1 = 第三视角背视，2 = 第三视角前视。
    /// 只在「我们自己按的 F5」上可信 —— 你手动按游戏内 F5 会让它漂，
    /// 所以给了 F10 当等价键（走同一套记账）。
    perspective: u8,
    /// 是否知道当前视角。不知道就先按满一圈归零再走到目标。
    perspective_known: bool,

    /* 前台门控 */
    /// 缓存的前台状态（100ms 刷一次，不必每拍问系统）
    fg: bool,
    next_fg_ms: u64,
    /// 当前是否因为失焦而暂停注入
    paused_bg: bool,
    /// 累计进入暂停的次数，跑完打印出来
    pub bg_pauses: u64,
    next_badge_ms: u64,
}

impl Agent {
    pub fn new(cfg: Config, input: Box<dyn Input>) -> Agent {
        let mut machine = Machine::new();
        let armed = false;
        machine.set_armed(armed);
        let c = cfg.channels;
        let channels = [
            c.movement,
            c.movement,
            c.look,
            c.look,
            c.attack,
            c.jump,
            c.use_item,
        ];
        Agent {
            machine,
            behaviors: Behaviors::new(&cfg),
            input,
            tail: None,
            sensor: None,
            sensor_at: 0,
            hp: None,
            threat: Tri::Zero,
            threat_until: 0,
            contact_until: 0,
            dead: false,
            dist_tier: 0,
            hazard_tier: 0,
            hostiles: None,
            strat: StratState::new(),
            squad: Squad::new(cfg.squad),
            gather: crate::gather::Gather::new(),
            dry_cmd: DrySink::default(),
            pos_last: None,
            pos_win_start: 0,
            pos_travel: 0.0,
            stuck: false,
            key_w: false,
            key_s: false,
            key_a: false,
            key_d: false,
            key_space: false,
            key_shift: false,
            btn_left: false,
            btn_right: false,
            btn_left_until: 0,
            btn_left_hold: false,
            btn_right_until: 0,
            space_until: 0,
            respawn_clicks: 0,
            yaw_px: 0.0,
            pitch_px: 0.0,
            next_recover_ms: 0,
            next_log_ms: 0,
            armed,
            channels,
            run_log: None,
            log_tick_stride: 20,
            perspective: 0,
            perspective_known: false,
            fg: true,
            next_fg_ms: 0,
            paused_bg: false,
            bg_pauses: 0,
            next_badge_ms: 0,
            cfg,
        }
    }

    pub fn attach_log(&mut self, tail: LogTail) {
        self.tail = Some(tail);
    }

    /// 开关 AI。**幂等** —— 已经是这个状态就什么都不做，免得每按一次
    /// 游戏内 `!ai on` 就重切一次视角。
    pub fn arm(&mut self, on: bool) {
        if self.armed == on {
            return;
        }
        self.armed = on;
        self.machine.set_armed(on);
        if !on {
            // 交还控制权必须当场松手，不能等下一拍：现在这一帧你还按着 W，
            // 下一帧就是「人手和 AI 抢键盘」。
            self.release_all();
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    // ------------------------------------------------------------ 视角

    /// 切到目标视角。0 = 第一，1 = 第三背视，2 = 第三前视。
    ///
    /// 账本可信时按最短差值走。**账本可能不可信** —— 你手动按了游戏内 F5、
    /// 或者上一次切换因为窗口不在前台没按成，账面就和画面对不上了。
    /// 所以要「保证落点正确」的地方一律用 [`Self::force_perspective`]。
    pub fn goto_perspective(&mut self, target: u8) {
        self.set_perspective(target, false);
    }

    /// 强制切到目标视角：不管账面说什么，先按满一圈归零再走到目标。
    /// 3 次 F5 一定回到第一视角，这是三态环的性质，不需要读当前档位。
    pub fn force_perspective(&mut self, target: u8) {
        self.set_perspective(target, true);
    }

    fn set_perspective(&mut self, target: u8, force: bool) {
        let target = target % 3;
        // force 等价于「假装不知道起点」，复用同一套步数计算
        let known = self.perspective_known && !force;
        let presses = perspective_steps(self.perspective, target, known);
        if presses > 0 && !self.press_f5(presses) {
            // 没按成（窗口不在前台）就不能记账 —— 否则账面说「已经在第一视角」，
            // 实际画面还停在第三视角，下次交还时一次都不按，人就卡在 AI 的视角里。
            self.perspective_known = false;
            return;
        }
        self.perspective = target;
        self.perspective_known = true;
    }

    /// 按 `times` 次 F5。返回 false 表示根本没按（不在前台）。
    fn press_f5(&mut self, times: u8) -> bool {
        // 不在前台时绝不能按：浏览器里 F5 是刷新，编辑器里是跳转。
        if !self.input.is_foreground() {
            return false;
        }
        for _ in 0..times {
            self.input.key(crate::act::vk::F5, true);
            std::thread::sleep(Duration::from_millis(55));
            self.input.key(crate::act::vk::F5, false);
            std::thread::sleep(Duration::from_millis(55));
        }
        true
    }

    /// AI 开关状态变化后同步视角：接管→第三视角，交还→第一视角。
    ///
    /// 两个方向都用**强制路径**（按满一圈再走）。理由很实际：
    /// 你可能在 AI 开着时自己按了游戏 F5 看风景，账面早就漂了；
    /// 这时候如果按差值算，交还时会停在第三视角而不是第一视角。
    /// 多按两下的代价（约 220ms）远小于「关都关了画面还在人物背后」。
    pub fn sync_view(&mut self) {
        if self.armed {
            if self.cfg.third_person_on_start {
                self.force_perspective(self.cfg.perspective_presses);
            }
        } else if self.cfg.restore_first_person {
            self.force_perspective(0);
        }
    }

    /// F10：和游戏内 F5 同效果，但走我们自己的记账路径。
    /// 你手动按游戏 F5 会让记账漂掉，按这个不会。
    pub fn cycle_perspective(&mut self) {
        let next = if self.perspective_known {
            (self.perspective + 1) % 3
        } else {
            1
        };
        self.goto_perspective(next);
    }

    /// 把「开/关」写到窗口标题上。
    fn update_badge(&mut self) {
        if !self.cfg.title_badge {
            return;
        }
        let t = if self.armed { "[AI 接管]" } else { "[AI 待机]" };
        self.input.badge(Some(t));
    }

    /// 「解开次操作」：把某一项交还给人类。
    pub fn set_channel(&mut self, ch: Channel, on: bool) {
        self.channels[ch.index()] = on;
        if !on {
            self.force_release(ch);
        }
    }

    pub fn toggle_channel(&mut self, ch: Channel) -> bool {
        let v = !self.channels[ch.index()];
        self.set_channel(ch, v);
        v
    }

    fn force_release(&mut self, ch: Channel) {
        match ch {
            Channel::Forward => {
                self.set_key_w(false);
                self.set_key_s(false);
            }
            Channel::Strafe => {
                self.set_key_a(false);
                self.set_key_d(false);
            }
            Channel::Attack => {
                if self.btn_left {
                    self.input.button(Button::Left, false);
                    self.btn_left = false;
                }
            }
            Channel::Jump => self.set_key_space(false),
            Channel::Use => {
                if self.btn_right {
                    self.input.button(Button::Right, false);
                    self.btn_right = false;
                }
            }
            Channel::Yaw | Channel::Pitch => {}
        }
    }

    pub fn release_all(&mut self) {
        self.input.release_all();
        self.key_w = false;
        self.key_s = false;
        self.key_a = false;
        self.key_d = false;
        self.key_space = false;
        self.key_shift = false;
        self.btn_left = false;
        self.btn_left_hold = false;
        self.btn_right = false;
    }

    fn set_key_w(&mut self, on: bool) {
        if self.key_w != on {
            self.key_w = on;
            self.input.key(crate::act::vk::W, on);
        }
    }
    fn set_key_s(&mut self, on: bool) {
        if self.key_s != on {
            self.key_s = on;
            self.input.key(crate::act::vk::S, on);
        }
    }
    fn set_key_a(&mut self, on: bool) {
        if self.key_a != on {
            self.key_a = on;
            self.input.key(crate::act::vk::A, on);
        }
    }
    fn set_key_d(&mut self, on: bool) {
        if self.key_d != on {
            self.key_d = on;
            self.input.key(crate::act::vk::D, on);
        }
    }
    fn set_key_space(&mut self, on: bool) {
        if self.key_space != on {
            self.key_space = on;
            self.input.key(crate::act::vk::SPACE, on);
        }
    }

    /// 卡住检测：按着前后键，但 0.9 秒内没能离开原处 0.4 格。
    ///
    /// 这是 `improvedmobs/BlockBreakGoal` 的触发条件在外部驱动侧的等价物：
    /// 那个 Goal 靠 `navigation.isStuck()`，我们只能靠坐标差分。
    ///
    /// 两个细节都是踩出来的：
    /// 1. **只在前后键按下时判定**。原先把横移键也算进去，结果角色原地左右
    ///    走位（Forward=o、Strafe 来回）时净位移接近 0，被误判成撞墙，然后
    ///    低头砍地。「卡住」的语义本来就是「想前进但前进不了」，横移不算。
    /// 2. 数据包 1 秒才报一次坐标，所以这里本质是「1 秒净位移」。想更准就得
    ///    提高上报频率，代价是聊天框刷屏 —— 现在这样够用。
    fn track_motion(&mut self, x: f32, y: f32, z: f32, now_ms: u64) {
        // 用 pos_last 而不是 pos_win_start==0 判断首帧：
        // now_ms 真的可能是 0，用 0 当哨兵会把第二个窗口整个吞掉。
        let first = self.pos_last.is_none();
        if let Some(prev) = self.pos_last {
            let dx = x - prev.0;
            let dz = z - prev.2;
            let step = (dx * dx + dz * dz).sqrt();
            if step.is_finite() {
                self.pos_travel += step;
            }
        }
        self.pos_last = Some((x, y, z));
        let _ = y;

        if first {
            self.pos_win_start = now_ms;
            return;
        }
        if now_ms.saturating_sub(self.pos_win_start) < STUCK_WINDOW_MS {
            return;
        }
        // 只认前后键：横移被挡住不算卡住
        let pushing = self.key_w || self.key_s;
        self.stuck = pushing && !self.dead && self.pos_travel < STUCK_MIN_TRAVEL;
        self.pos_travel = 0.0;
        self.pos_win_start = now_ms;
    }

    /// 处理一条游戏内事件。
    pub fn ingest(&mut self, ev: LogEvent, now_ms: u64) {
        match ev {
            LogEvent::Sensor(s) => {
                self.hp = Some(s.hp);
                self.dead = s.hp <= 0.0;
                self.dist_tier = s.dist_tier;
                self.hazard_tier = s.hazard_tier;
                self.hostiles = Some(s.hostiles.max(0) as u32);
                // 敌人数和距离档任一非零都算有威胁：数据包的计数有半径上限，
                // 而距离档是独立的几档判定，两边取并集才不会漏。
                if s.hostiles > 0 || s.dist_tier > 0 {
                    self.threat = Tri::Pos;
                    self.threat_until = now_ms + 2500;
                    self.contact_until = now_ms + 4000;
                } else {
                    self.threat = Tri::Neg;
                    self.threat_until = now_ms + 1200;
                }
                if s.hp > 0.5 {
                    self.respawn_clicks = 0;
                }
                self.track_motion(s.x, s.y, s.z, now_ms);
                self.sensor_at = now_ms;
                self.sensor = Some(s);
            }
            LogEvent::HealthProbe(h) => {
                self.hp = Some(h);
                self.dead = h <= 0.0;
            }
            LogEvent::Death(_) => {
                self.dead = true;
                // 死过一次不代表现在还在交战：威胁降级为「未知」，等传感器重新报
                self.threat = Tri::Zero;
                self.threat_until = 0;
                self.contact_until = 0;
                // 死了就别再指挥宠物了：这时候发指令只会在死亡界面里乱敲
                self.hostiles = None;
            }
            LogEvent::Join(_) => {
                self.dead = false;
                self.hp = None;
            }
            LogEvent::Chat { text, .. } => {
                if let Some(cmd) = text.strip_prefix('!') {
                    self.command(cmd.trim(), now_ms);
                }
            }
            _ => {}
        }
    }

    /// 游戏内聊天指令：`!ai on` / `!mode patrol` / `!attack off` / `!panic`
    pub fn command(&mut self, cmd: &str, now_ms: u64) -> String {
        let mut it = cmd.split_whitespace();
        let head = it.next().unwrap_or("").to_ascii_lowercase();
        let arg = it.next().unwrap_or("").to_ascii_lowercase();
        let on = !matches!(arg.as_str(), "off" | "0" | "false" | "关");
        let reply = match head.as_str() {
            "ai" | "on" | "off" => {
                let v = if head == "off" { false } else { on };
                self.arm(v);
                format!("ai {}", if v { "on" } else { "off" })
            }
            "panic" | "stop" => {
                self.arm(false);
                self.release_all();
                "panic: released".to_string()
            }
            "mode" => {
                self.cfg.mode = Mode::parse(&arg);
                self.behaviors = Behaviors::new(&self.cfg);
                format!("mode {}", self.cfg.mode.as_str())
            }
            "attack" => {
                self.set_channel(Channel::Attack, on);
                format!("attack {}", if on { "on" } else { "off" })
            }
            "move" => {
                self.set_channel(Channel::Forward, on);
                self.set_channel(Channel::Strafe, on);
                format!("move {}", if on { "on" } else { "off" })
            }
            "look" => {
                self.set_channel(Channel::Yaw, on);
                self.set_channel(Channel::Pitch, on);
                format!("look {}", if on { "on" } else { "off" })
            }
            "jump" => {
                self.set_channel(Channel::Jump, on);
                format!("jump {}", if on { "on" } else { "off" })
            }
            "use" => {
                self.set_channel(Channel::Use, on);
                format!("use {}", if on { "on" } else { "off" })
            }
            "threat" => {
                self.threat = Tri::Pos;
                self.contact_until = now_ms + 8000;
                "threat marked".to_string()
            }
            "calm" => {
                self.threat = Tri::Neg;
                self.contact_until = 0;
                "calm".to_string()
            }
            "tac" => {
                let t = &mut self.cfg.tactics;
                match arg.as_str() {
                    "" => {}
                    "on" | "1" => *t = crate::config::Tactics::default(),
                    "off" | "0" => {
                        *t = crate::config::Tactics {
                            range: false,
                            hazard: false,
                            guard: false,
                            breach: false,
                        }
                    }
                    "guard" => t.guard = !t.guard,
                    "range" => t.range = !t.range,
                    "hazard" => t.hazard = !t.hazard,
                    "breach" => t.breach = !t.breach,
                    _ => {}
                }
                self.behaviors = Behaviors::new(&self.cfg);
                self.tactics_line()
            }
            "status" => format!(
                "{} phase={} hp={} {} {} {} {}",
                if self.armed { "armed" } else { "off" },
                self.machine.phase.name(),
                self.hp.map(|h| format!("{h:.1}")).unwrap_or("?".into()),
                self.tactics_line(),
                self.strategy_line(),
                self.squad.line(self.cfg.squad_control.as_str()),
                self.gather.line()
            ),
            "squad" => match arg.as_str() {
                "" => self.squad.line(self.cfg.squad_control.as_str()),
                "on" | "1" => {
                    self.cfg.squad.enabled = true;
                    self.squad.cfg.enabled = true;
                    "squad on".to_string()
                }
                "off" | "0" => {
                    self.cfg.squad.enabled = false;
                    self.squad.cfg.enabled = false;
                    self.squad.set_manual(None);
                    "squad off".to_string()
                }
                "auto" => {
                    self.squad.set_manual(None);
                    "squad auto".to_string()
                }
                other => match crate::squad::Intent::parse(other) {
                    Some(i) => {
                        self.squad.set_manual(Some(i));
                        format!("squad manual {}", i.name())
                    }
                    None => "? !squad on|off|auto|hold|engage|regroup|recall".to_string(),
                },
            },
            "strat" => match arg.as_str() {
                "" => self.strategy_line(),
                "on" | "1" => {
                    self.cfg.strategies = crate::strategy::Strategies {
                        feint_ms: self.cfg.strategies.feint_ms,
                        ..Default::default()
                    };
                    self.strategy_line()
                }
                "off" | "0" => {
                    self.cfg.strategies.retreat = false;
                    self.cfg.strategies.wait = false;
                    self.cfg.strategies.blitz = false;
                    self.cfg.strategies.ambush = false;
                    self.cfg.strategies.feint = false;
                    "strat all off".to_string()
                }
                other => {
                    let s = &mut self.cfg.strategies;
                    match other {
                        "retreat" => s.retreat = !s.retreat,
                        "wait" => s.wait = !s.wait,
                        "blitz" => s.blitz = !s.blitz,
                        "ambush" => s.ambush = !s.ambush,
                        "feint" => s.feint = !s.feint,
                        _ => {}
                    }
                    self.strategy_line()
                }
            },
            "gather" => match arg.as_str() {
                "" => self.gather.line(),
                "on" | "1" => {
                    self.cfg.gather.enabled = true;
                    format!("gather on ({})", self.gather.line())
                }
                "off" | "0" => {
                    self.cfg.gather.enabled = false;
                    self.gather.reset();
                    "gather off".to_string()
                }
                // 清配额和冷却：卡住了不想等 30 秒就敲这个
                "reset" => {
                    self.gather.reset();
                    "gather reset".to_string()
                }
                "night" => {
                    self.cfg.gather.allow_night = !self.cfg.gather.allow_night;
                    format!(
                        "gather night {}",
                        if self.cfg.gather.allow_night { "on" } else { "off" }
                    )
                }
                other => {
                    if let Ok(v) = other.parse::<u32>() {
                        self.cfg.gather.quota = v.min(1_000);
                        format!("gather quota {}", self.cfg.gather.quota)
                    } else {
                        "? !gather on|off|reset|night|<配额>".to_string()
                    }
                }
            },
            _ => "? !ai !mode !tac !strat !squad !gather !attack !move !look !jump !use !threat !calm !status"
                .to_string(),
        };
        reply
    }

    /// 当前生效的战法一行摘要。
    fn strategy_line(&self) -> String {
        let s = &self.cfg.strategies;
        let on = |b: bool| if b { "+" } else { "-" };
        format!(
            "strat={}{}{}{}{} 生效={}",
            on(s.retreat),
            on(s.wait),
            on(s.blitz),
            on(s.ambush),
            on(s.feint),
            match self.strat.active {
                Some(x) => x.name(),
                None => "-",
            }
        )
    }

    /// 战术开关的一行摘要，给 `!status` 和 `!tac` 用。
    fn tactics_line(&self) -> String {
        let t = &self.cfg.tactics;
        let on = |b: bool| if b { "+" } else { "-" };
        format!(
            "tac={}{}{}{}",
            on(t.range),
            on(t.hazard),
            on(t.guard),
            on(t.breach)
        )
    }

    pub fn world(&self, tick: u64, now_ms: u64) -> WorldView {
        let dist = crate::npc::tier_to_dist(self.dist_tier);
        // 数据包只报「最近爆炸源」一档，统一按苦力怕的 10 格判据处理：
        // TNT 引信 4 秒、爆炸半径 7 格，10 格起跑更保守。
        let hazard = crate::npc::hazard_band(crate::npc::tier_to_dist(self.hazard_tier), None);
        // 传感器是有新鲜度的：数据包每秒报一次，停更 3 秒说明游戏那边
        // 出问题了（卡死 / 关掉了 / 退出世界）。这时候宁可让「木头位置」变成
        // 未知，也不能拿旧坐标一路砍 —— 会对着空气挥 20 秒。
        let fresh = |now_ms: u64| -> Option<&Sensor> {
            self.sensor
                .as_ref()
                .filter(|_| now_ms.saturating_sub(self.sensor_at) < 3_000)
        };
        let sn = fresh(now_ms);

        WorldView {
            hp: self.hp.map(|h| (h / 20.0).clamp(0.0, 1.0)),
            threat: self.threat,
            contact: self.contact_until > now_ms,
            dist,
            hazard,
            enemies: self.hostiles,
            food: sn.map(|s| s.food),
            wood: sn.and_then(|s| crate::gather::wood_dist(s.wood_tier)),
            wood_dir: sn.map(|s| s.wood_dir),
            night: sn.and_then(|s| s.is_night()),
            pitch: sn.map(|s| s.pitch),
            stuck: self.stuck,
            pos: self.pos_last,
            dead: self.dead,
            human_takeover: !self.armed,
            tick,
        }
    }

    pub fn step(&mut self, now_ms: u64, dt_ms: u64) -> Decision {
        // 威胁/接触衰减
        if self.threat_until > 0 && now_ms > self.threat_until {
            self.threat = Tri::Zero;
            self.threat_until = 0;
        }
        if self.contact_until > 0 && now_ms > self.contact_until {
            self.contact_until = 0;
        }

        let mut w = self.world(self.machine.ticks, now_ms);
        // 无感知时的显式「无脑模式」：只有配置里自己开了才允许猜
        if self.cfg.assume_threat && self.sensor.is_none() && self.armed {
            w.threat = Tri::Pos;
            w.contact = true;
        }
        if !self.armed {
            w.human_takeover = true;
        }
        // 先 peek 出本周期阶段，行为模块据此投票；再由 step 用同一输入重算
        let phase_now = self.machine.peek(&w);
        // 采集门控先跑：行为层只负责"手怎么动"，"该不该动手"在这里定。
        // 注意即使不在 Idle 也照跑 —— 交战期间正是该累积冷却的时候，
        // 不然打完架立刻回去砍，等于没被打断过。
        let gmode = self.gather.tick(&w, &self.cfg.gather, now_ms);
        let ctx = Ctx {
            w,
            phase: phase_now,
            now_ms,
            dt_ms,
            gather: gmode,
        };

        // 共同意识先跑：让从属实体跟上同一份判断。
        // 放在行为投票之前，是因为它可能要打字发指令（阻塞 ~300ms），
        // 打完字按键状态必须重置，否则这一拍的 apply 会以为键还按着。
        self.drive_squad(&w, now_ms);

        let mut votes: Vec<Vote> = self.behaviors.vote(&ctx);
        // 战法层只追加票，不改别人投的票
        let active = crate::strategy::select(&w, &self.cfg.strategies);
        crate::strategy::apply(
            active,
            &mut votes,
            now_ms,
            &mut self.strat,
            &self.cfg.strategies,
        );
        let d = self.machine.step(&w, &votes, &self.channels);
        self.apply(&d, now_ms, dt_ms);
        d
    }

    /// 把当前意图下发给从属实体（傀儡 / 狼）。
    fn drive_squad(&mut self, w: &WorldView, now_ms: u64) {
        match self.cfg.squad_control {
            ControlChannel::Off => {
                // 通道关着也要更新意图，`!squad` 才能看到 AI 打算怎么指挥
                self.squad.plan(w);
            }
            ControlChannel::Dry => {
                let fired = self.squad.tick(w, now_ms, &mut self.dry_cmd);
                if fired {
                    let last = self.dry_cmd.log.last().cloned().unwrap_or_default();
                    println!("[squad] {last}");
                }
            }
            ControlChannel::Chat => {
                let mut sink = InputSink(&mut *self.input);
                let fired = self.squad.tick(w, now_ms, &mut sink);
                if fired {
                    // 打字过程中 input 层已经松开所有键，这里要把 Agent 自己的
                    // 「我以为按着」也清掉，否则下一拍 set_key_w(true) 会因为
                    // key_w 已经是 true 而不再真的按键 —— 角色就不动了。
                    self.reset_key_state();
                }
            }
        }
    }

    /// 忘掉「按键状态」的缓存，让下一拍 apply 重新按一遍。
    fn reset_key_state(&mut self) {
        self.key_w = false;
        self.key_s = false;
        self.key_a = false;
        self.key_d = false;
        self.key_space = false;
        self.key_shift = false;
        self.btn_left = false;
        self.btn_right = false;
    }

    /// `squad_control = dry` 时已经下发的指令（给 `!squad` 和测试用）。
    pub fn squad_log(&self) -> &[String] {
        &self.dry_cmd.log
    }

    /// 当前生效的战法（给 `!strat` 和测试用）。
    pub fn active_strategy(&self) -> Option<crate::strategy::Strat> {
        self.strat.active
    }

    /// 前台门控：目标窗口不在前台就一律不注入。
    ///
    /// 这不是「优化」，是防止事故。`SendInput` 是系统级注入，游戏失焦后
    /// 注入照样生效 —— 你 Alt+Tab 去看个网页，AI 就往浏览器里敲 w、点左键、
    /// 甚至按 F5 刷新（所以按 F5 前也单查了一次前台）。
    fn gate(&mut self, now_ms: u64) -> bool {
        if !self.cfg.pause_when_unfocused {
            return true;
        }
        if now_ms >= self.next_fg_ms {
            self.next_fg_ms = now_ms + 100;
            self.fg = self.input.is_foreground();
        }
        if self.fg {
            self.paused_bg = false;
            return true;
        }
        if !self.paused_bg {
            self.paused_bg = true;
            self.bg_pauses += 1;
            self.release_all();
        }
        false
    }

    fn apply(&mut self, d: &Decision, now_ms: u64, dt_ms: u64) {
        if !self.gate(now_ms) {
            return;
        }
        // 前后
        match d.dirs[Channel::Forward.index()] {
            Tri::Pos => {
                self.set_key_s(false);
                self.set_key_w(true);
            }
            Tri::Neg => {
                self.set_key_w(false);
                self.set_key_s(true);
            }
            Tri::Zero => {
                self.set_key_w(false);
                self.set_key_s(false);
            }
        }
        // 左右
        match d.dirs[Channel::Strafe.index()] {
            Tri::Pos => {
                self.set_key_a(false);
                self.set_key_d(true);
            }
            Tri::Neg => {
                self.set_key_d(false);
                self.set_key_a(true);
            }
            Tri::Zero => {
                self.set_key_a(false);
                self.set_key_d(false);
            }
        }
        // 视角：三值只给方向，速度是连续量
        let dt_s = dt_ms as f32 / 1000.0;
        let deg = self.cfg.look_rate * dt_s;
        if d.dirs[Channel::Yaw.index()] != Tri::Zero {
            self.yaw_px += deg * d.dirs[Channel::Yaw.index()].to_f32() * self.cfg.look_gain;
            let dx = self.yaw_px as i32;
            if dx != 0 {
                self.yaw_px -= dx as f32;
                self.input.mouse_move(dx, 0);
            }
        }
        if d.dirs[Channel::Pitch.index()] != Tri::Zero {
            let sign = if self.cfg.invert_pitch { -1.0 } else { 1.0 };
            self.pitch_px += deg * d.dirs[Channel::Pitch.index()].to_f32() * self.cfg.look_gain * sign;
            let dy = self.pitch_px as i32;
            if dy != 0 {
                self.pitch_px -= dy as f32;
                self.input.mouse_move(0, dy);
            }
        }
        // 左键。两种节奏，靠 decision 区分：
        // - 脉冲（打怪）：按下后至少按住 MIN_CLICK_MS，保证跨过一帧采样；
        // - 按住（砍方块）：一直按着，直到决策自己撤回 —— 挖矿进度是连续的，
        //   松手就归零，脉冲式永远砍不断。
        if d.dirs[Channel::Attack.index()] == Tri::Pos {
            if !self.btn_left {
                self.btn_left = true;
                self.input.button(Button::Left, true);
            }
            self.btn_left_hold = d.attack_hold;
            self.btn_left_until = now_ms + MIN_CLICK_MS;
        } else if self.btn_left {
            // 按住模式一看到决策不再要攻击，立刻松 —— 不能等超时，
            // 否则扫视那 350ms 里还在白砍。
            if self.btn_left_hold || now_ms >= self.btn_left_until {
                self.btn_left = false;
                self.btn_left_hold = false;
                self.input.button(Button::Left, false);
            }
        }
        // 右键
        if d.dirs[Channel::Use.index()] == Tri::Pos {
            if !self.btn_right {
                self.btn_right = true;
                self.input.button(Button::Right, true);
            }
            self.btn_right_until = now_ms + MIN_CLICK_MS;
        } else if self.btn_right && now_ms >= self.btn_right_until {
            self.btn_right = false;
            self.input.button(Button::Right, false);
        }
        // 跳
        if d.dirs[Channel::Jump.index()] == Tri::Pos {
            if !self.key_space {
                self.key_space = true;
                self.input.key(crate::act::vk::SPACE, true);
            }
            self.space_until = now_ms + MIN_JUMP_MS;
        } else if self.key_space && now_ms >= self.space_until {
            self.key_space = false;
            self.input.key(crate::act::vk::SPACE, false);
        }
        // 疾跑：前进时按住 Shift
        let running = self.cfg.sprint && d.dirs[Channel::Forward.index()] == Tri::Pos;
        if running != self.key_shift {
            self.key_shift = running;
            self.input.key(crate::act::vk::SHIFT, running);
        }
        // 死亡：点重生
        if d.recover_click && now_ms >= self.next_recover_ms {
            self.next_recover_ms = now_ms + self.cfg.respawn_click_ms;
            self.respawn_clicks = self.respawn_clicks.saturating_add(1);
            if self.respawn_clicks > MAX_RESPAWN_CLICKS {
                // 点了十几次都没活过来，大概率是画面被别的界面挡住（Esc 菜单 / 死亡界面没出）。
                // 继续盲点只会误触，直接交还控制权。
                self.arm(false);
                return;
            }
            self.input.click_center();
            // 点了重生不等于回满血，血量仍然是未知，等传感器报
            self.dead = false;
            self.hp = None;
        }
    }

    pub fn open_run_log(&mut self) {
        if let Ok(f) = std::fs::File::create(&self.cfg.run_log) {
            self.run_log = Some(f);
        }
    }

    fn log_decision(&mut self, now_ms: u64, d: &Decision) {
        if self.run_log.is_none() {
            return;
        }
        if now_ms < self.next_log_ms {
            return;
        }
        self.next_log_ms = now_ms + 100;
        if let Some(f) = self.run_log.as_mut() {
            let _ = writeln!(
                f,
                "{} {} hp={}",
                now_ms,
                d.encode(),
                self.hp.map(|h| format!("{h:.1}")).unwrap_or_else(|| "?".into())
            );
        }
    }

    /// 主循环。`hotkeys` 为 None 时不做热键检测（仿真用）。
    #[cfg(windows)]
    pub fn run(
        &mut self,
        mut hotkeys: Option<crate::act::win32::Hotkeys>,
        duration: Option<Duration>,
    ) -> RunStats {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1.0 / self.cfg.hz as f64);
        let mut last = start;
        let mut ticks: u64 = 0;
        let mut attacks: u64 = 0;
        let mut phase_hist = [0u64; 5];
        let mut last_log = start;

        let kv = self.cfg.kill_switch_vk;
        let pv = self.cfg.panic_vk;
        let av = self.cfg.toggle_attack_vk;
        let mv = self.cfg.toggle_move_vk;
        let tv = self.cfg.toggle_perspective_vk;
        let mut last_armed = self.armed;

        // 启动：抢焦点 → 挂状态标记 → 切到接管视角。
        // 抢不到焦点不做任何注入，否则键全打到别的窗口上。
        if !self.input.focus() {
            println!(
                "警告：没能把游戏窗口拉到前台。切回游戏后 AI 才会动手（pause_when_unfocused={}）。",
                self.cfg.pause_when_unfocused
            );
        }
        std::thread::sleep(Duration::from_millis(150));
        self.update_badge();
        self.sync_view();

        loop {
            let now = Instant::now();
            let now_ms = now.duration_since(start).as_millis() as u64;
            let dt_ms = now.duration_since(last).as_millis().max(1) as u64;
            last = now;

            if let Some(hk) = hotkeys.as_mut() {
                for k in hk.poll() {
                    if k == kv {
                        let on = !self.armed;
                        self.arm(on);
                    } else if k == pv {
                        self.arm(false);
                        break;
                    } else if k == av {
                        self.toggle_channel(Channel::Attack);
                    } else if k == mv {
                        let on = !self.channels[Channel::Forward.index()];
                        self.set_channel(Channel::Forward, on);
                        self.set_channel(Channel::Strafe, on);
                    } else if k == tv {
                        self.cycle_perspective();
                    }
                }
            }

            // 开关状态变了：标题标记 + 视角跟着切（接管→第三视角，交还→第一视角）
            if self.armed != last_armed {
                last_armed = self.armed;
                self.update_badge();
                self.sync_view();
                // 全屏时标题栏根本看不见，控制台这一行是唯一还能看的反馈
                println!(
                    "{}",
                    if self.armed {
                        "▶ AI 接管（F8 交还控制权）"
                    } else {
                        "■ 已交还控制权（F8 重新接管）"
                    }
                );
            }
            // 游戏自己也会改标题（切世界/进服务器），定期重挂一次
            if self.cfg.title_badge && now_ms >= self.next_badge_ms {
                self.next_badge_ms = now_ms + 500;
                self.input.refresh_badge();
            }

            // 日志不用每 tick 读，50ms 一次足够
            if now.duration_since(last_log) >= Duration::from_millis(50) {
                last_log = now;
                let events = match self.tail.as_mut() {
                    Some(t) => t.poll(),
                    None => Vec::new(),
                };
                for ev in events {
                    self.ingest(ev, now_ms);
                }
            }

            let d = self.step(now_ms, dt_ms);
            if d.dirs[Channel::Attack.index()] == Tri::Pos {
                attacks += 1;
            }
            let pi = match d.phase {
                Phase::Off => 0,
                Phase::Idle => 1,
                Phase::Engage => 2,
                Phase::Retreat => 3,
                Phase::Recover => 4,
            };
            phase_hist[pi] += 1;
            if ticks % self.log_tick_stride == 0 {
                self.log_decision(now_ms, &d);
            }
            ticks += 1;

            if let Some(dur) = duration {
                if now.duration_since(start) >= dur {
                    break;
                }
            }

            // 定速：睡到下一拍，最后 1ms 自旋（Windows sleep 精度不够）
            let elapsed = Instant::now().duration_since(start);
            let target = period.mul_f64(ticks as f64);
            if elapsed < target {
                let remain = target - elapsed;
                if remain > Duration::from_millis(2) {
                    std::thread::sleep(remain - Duration::from_millis(1));
                }
                while Instant::now().duration_since(start) < target {
                    std::hint::spin_loop();
                }
            } else if elapsed > target + period.mul_f64(4.0) {
                // 落后太多，重新对齐，避免追帧雪崩
                ticks = (elapsed.as_secs_f64() * self.cfg.hz as f64) as u64;
            }
        }

        self.release_all();
        // 退出时别把标记留在标题上，也别把人锁在第三视角里
        if self.cfg.title_badge {
            self.input.badge(None);
        }
        if self.armed && self.cfg.restore_first_person {
            self.force_perspective(0);
        }
        let elapsed = start.elapsed();
        RunStats {
            ticks,
            elapsed,
            achieved_hz: ticks as f64 / elapsed.as_secs_f64().max(1e-9),
            attacks,
            decisions: ticks,
            phase_hist,
            exits: 0,
            bg_pauses: self.bg_pauses,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一条传感器事件：hp / 敌人数 / 最近敌人距离档。
    fn sens(hp: f32, hostiles: i32, d: i32) -> LogEvent {
        sens_at(hp, hostiles, d, 0.0, 0.0, 0)
    }

    /// 同上，但指定坐标和爆炸源档。
    fn sens_at(hp: f32, hostiles: i32, d: i32, x: f32, z: f32, c: i32) -> LogEvent {
        LogEvent::Sensor(Sensor {
            hp,
            food: 17,
            x,
            y: 64.0,
            z,
            hostiles,
            dist_tier: if hostiles > 0 { d } else { 0 },
            hazard_tier: c,
            wood_tier: 0,
            wood_dir: 0,
            tod: 8_000,
            pitch: 0.0,
        })
    }

    /// 指定「附近有没有木头」的传感器事件。`tier` 用数据包的档位：0/3/6/12。
    fn sens_wood(hp: f32, tier: i32, dir: i32, tod: i32) -> LogEvent {
        match sens(hp, 0, 0) {
            LogEvent::Sensor(mut s) => {
                s.wood_tier = tier;
                s.wood_dir = dir;
                s.tod = tod;
                LogEvent::Sensor(s)
            }
            other => other,
        }
    }
    use crate::act::dry::DryInput;

    fn agent() -> Agent {
        let mut cfg = Config::default();
        cfg.assume_threat = false;
        Agent::new(cfg, Box::new(DryInput::new()))
    }

    #[test]
    fn sensors_drive_engage_phase() {
        let mut a = agent();
        a.arm(true);
        a.ingest(
            sens(20.0, 3, 3),
            0,
        );
        let d = a.step(100, 5);
        assert_eq!(d.phase, Phase::Engage);
    }

    #[test]
    fn no_sensor_stays_idle_and_abstains() {
        let mut a = agent();
        a.arm(true);
        let d = a.step(100, 5);
        assert_eq!(d.phase, Phase::Idle);
        assert_eq!(d.reason, crate::fsm::Reason::Unknown);
    }

    #[test]
    fn chat_command_toggles_ai() {
        let mut a = agent();
        a.arm(true);
        a.command("attack off", 0);
        assert!(!a.channels[Channel::Attack.index()]);
        a.command("ai off", 0);
        assert!(!a.is_armed());
        let d = a.step(10, 5);
        assert_eq!(d.phase, Phase::Off);
    }

    #[test]
    fn click_survives_multiple_ticks() {
        // 回归：20 TPS 下 5ms 的按键脉冲会被整帧漏掉，表现是「AI 不打人」
        let mut a = agent();
        a.arm(true);
        a.ingest(
            sens(20.0, 2, 3),
            0,
        );
        a.step(100, 5);
        assert!(a.btn_left, "第一拍应该按下左键");
        a.step(110, 5);
        assert!(a.btn_left, "下一拍没投票也必须保持按住");
        a.step(MIN_CLICK_MS + 120, 5);
        assert!(!a.btn_left, "超过最小按住时长后应松开");
    }

    #[test]
    fn wall_pushing_triggers_breach() {
        // 想追但坐标不动：1 秒后应判定卡住，然后低头砍墙。
        let mut a = agent();
        a.arm(true);
        a.ingest(sens_at(20.0, 1, 32, 0.0, 0.0, 0), 0);
        a.step(100, 5); // 这一拍按下 W，key_w = true
        assert!(a.key_w, "追击时应按住前进");
        a.ingest(sens_at(20.0, 1, 32, 0.0, 0.0, 0), 1_000);
        assert!(a.world(2, 1_000).stuck, "1 秒原地踏步应判定卡住");
        let d = a.step(1_100, 5);
        assert_eq!(d.dirs[Channel::Pitch.index()], Tri::Pos, "卡住应低头");
        assert_eq!(d.dirs[Channel::Attack.index()], Tri::Pos, "卡住应砍方块");
    }

    #[test]
    fn sidestepping_is_not_stuck() {
        // 原地横移不是卡住：净位移接近 0，但按的不是前后键
        let mut a = agent();
        a.arm(true);
        // 敌人 4.5 格 → Forward 站住、Strafe 来回
        a.ingest(sens_at(20.0, 1, 6, 0.0, 0.0, 0), 0);
        a.step(100, 5);
        assert!(!a.key_w, "中距离应站住不前进");
        a.ingest(sens_at(20.0, 1, 6, 0.0, 0.0, 0), 1_000);
        assert!(!a.world(2, 1_000).stuck, "没按前后键就不该判卡住");
    }

    // ---------------------------------------------------------------- 采集

    /// 开好采集的 agent。默认 `mode = hold`：不巡逻，免得跟采集抢前进票。
    fn forager() -> Agent {
        let cfg = Config::from_str("hz = 200
gather = true
mode = hold
");
        let mut a = Agent::new(cfg, Box::new(DryInput::new()));
        a.arm(true);
        a
    }

    /// 推进一段时间，每 500ms 补一次传感器（真实数据包是 1 秒一报，
    /// 而 `world()` 会把超过 3 秒的数据当过期）。
    ///
    /// `wood` 是这一段时间内传感器报的木头档位：0 = 视野内没有。
    fn pump(a: &mut Agent, from: u64, to: u64, wood: i32) -> Decision {
        let mut t = from;
        let mut d = a.step(t, 5);
        while t < to {
            if t % 500 == 0 {
                a.ingest(sens_wood(20.0, wood, 0, 8_000), t);
            }
            t += 50;
            d = a.step(t, 5);
        }
        d
    }

    #[test]
    fn gather_holds_left_button_while_chopping() {
        let mut a = forager();
        // 木头 3 格内 = 直接开砍（reach 默认 3.2）
        let d = pump(&mut a, 0, 4_000, 3);
        assert_eq!(a.gather.mode, crate::gather::Mode::Chop, "why={:?}", a.gather.why);
        assert_eq!(d.dirs[Channel::Attack.index()], Tri::Pos);
        assert!(d.attack_hold, "砍方块必须按住，脉冲式永远砍不断");
        assert!(a.btn_left && a.btn_left_hold);
    }

    #[test]
    fn gather_walks_to_a_tree_that_is_farther() {
        let mut a = forager();
        pump(&mut a, 0, 4_000, 6);
        assert_eq!(a.gather.mode, crate::gather::Mode::Approach);
        assert!(a.key_w, "6 格就该往那边走");
        assert!(!a.btn_left, "还没到就砍是白挥");
    }

    #[test]
    fn threat_makes_it_drop_everything_mid_chop() {
        let mut a = forager();
        pump(&mut a, 0, 4_000, 3);
        assert!(a.btn_left, "前提：已经在砍了");

        // 突然冒怪
        a.ingest(sens_at(20.0, 2, 6, 0.0, 0.0, 0), 4_000);
        let d = a.step(4_050, 5);
        assert_eq!(d.phase, Phase::Engage);
        // 注意不能断言 `!btn_left`：交战那一拍 Combat 会自己投攻击票（打怪）。
        // 要断言的是"不再以按住模式按着" —— 砍树的持续按住必须停。
        assert!(!a.btn_left_hold, "交战了还按着左键砍树就是白挨打");
        assert!(!d.attack_hold);
        assert_eq!(a.gather.mode, crate::gather::Mode::Cooldown);
        assert_eq!(a.gather.interrupted_by, crate::gather::Why::Threat);
    }

    #[test]
    fn cooldown_survives_the_enemy_leaving() {
        // 怪物走掉之后不能立刻回去砍 —— 这正是"原地抽搐"的根源
        let mut a = forager();
        pump(&mut a, 0, 4_000, 3);
        a.ingest(sens_at(20.0, 2, 6, 0.0, 0.0, 0), 4_000);
        a.step(4_050, 5);

        // 怪没了，但冷却还没过
        pump(&mut a, 4_100, 8_000, 3);
        assert_eq!(a.gather.mode, crate::gather::Mode::Cooldown);
        assert!(!a.btn_left_hold, "冷却期间不许按住左键");

        // 冷却过了才回去砍
        pump(&mut a, 8_100, 20_000, 3);
        assert_eq!(a.gather.mode, crate::gather::Mode::Chop, "why={:?}", a.gather.why);
    }

    #[test]
    fn night_blocks_gathering() {
        let cfg = Config::from_str("gather = true
");
        assert!(!cfg.gather.allow_night, "默认夜里不砍");

        let cfg = Config::from_str("hz = 200
gather = true
mode = hold
");
        let mut a = Agent::new(cfg, Box::new(DryInput::new()));
        a.arm(true);
        let mut t = 0;
        while t < 4_000 {
            a.ingest(sens_wood(20.0, 3, 0, 16_000), t); // 夜里
            a.step(t, 5);
            t += 50;
        }
        assert_eq!(a.gather.why, crate::gather::Why::Night);
        assert!(!a.btn_left);
    }

    #[test]
    fn no_sensor_means_no_gathering() {
        // 没装数据包 → 不知道有没有木头、也不知道是不是安全 → 一律不采
        let mut a = forager();
        for t in (0..4_000).step_by(50) {
            a.step(t, 5);
        }
        // 威胁状态从头到尾是未知（Zero），门控给出的就是"不确定"
        assert_ne!(a.gather.gate, Tri::Pos);
        assert!(!a.btn_left_hold);
    }

    #[test]
    fn stale_sensor_stops_gathering() {
        // 数据包停更（退世界 / 游戏卡死）后不能拿旧位置一路砍
        let mut a = forager();
        pump(&mut a, 0, 4_000, 3);
        assert!(a.btn_left);
        // 4 秒不再上报 → 超过 3 秒新鲜度
        for t in (4_100..9_000).step_by(50) {
            a.step(t, 5);
        }
        assert!(!a.btn_left_hold, "传感器过期了还在砍 = 对着空气挥");
        // 过期后木头位置和威胁状态都变成未知，两个理由都该拦住它
        assert!(
            matches!(
                a.gather.why,
                crate::gather::Why::NoWood | crate::gather::Why::ThreatUnknown
            ),
            "{:?}",
            a.gather.why
        );
    }

    #[test]
    fn gather_off_by_default() {
        let a = Agent::new(Config::default(), Box::new(DryInput::new()));
        assert!(!a.cfg.gather.enabled, "采集会砍掉用户的树，默认必须关");
    }

    #[test]
    fn gather_command_toggles_and_reports() {
        let mut a = forager();
        assert!(a.command("gather", 0).contains("gather="));
        assert!(a.command("gather off", 0).contains("off"));
        assert!(!a.cfg.gather.enabled);
        assert!(a.command("gather on", 0).contains("on"));
        assert!(a.cfg.gather.enabled);
        assert!(a.command("gather 8", 0).contains("quota 8"));
        assert_eq!(a.cfg.gather.quota, 8);
        assert!(a.command("gather night", 0).contains("night on"));
        assert!(a.cfg.gather.allow_night);
    }

    #[test]
    fn panic_releases_everything() {
        let mut a = agent();
        a.arm(true);
        a.ingest(
            sens(20.0, 1, 3),
            0,
        );
        a.step(50, 5);
        a.command("panic", 60);
        assert!(!a.is_armed());
        assert!(!a.key_w && !a.key_s && !a.btn_left);
    }

    // ---------------------------------------------------------- 视角记账

    #[test]
    fn perspective_known_start_uses_shortest_ring_step() {
        // 已知档位时只按差值：第一 → 背视就是 1 次
        assert_eq!(perspective_steps(0, 1, true), 1);
        assert_eq!(perspective_steps(1, 2, true), 1);
        // 环状回绕：前视 → 第一 只要 1 次，不是 2 次
        assert_eq!(perspective_steps(2, 0, true), 1);
        // 已经在目标档位就一次都别按
        assert_eq!(perspective_steps(1, 1, true), 0);
    }

    #[test]
    fn perspective_unknown_start_walks_a_full_ring() {
        // 不知道开局在哪一档：按满一圈（3）加上目标偏移，落点必然确定。
        // 这是「多按两下换确定性」的核心，不能改成猜。
        for target in 0..3u8 {
            assert_eq!(
                perspective_steps(0, target, false),
                3 + target,
                "未知起点切到 {target} 档必须走满一圈"
            );
        }
        // 未知时结果与 cur 无关 —— 这正是它可靠的原因
        assert_eq!(perspective_steps(0, 1, false), perspective_steps(2, 1, false));
    }

    // ---------------------------------------------------------- 失焦门控

    #[test]
    fn unfocused_window_gets_no_injection() {
        // Alt+Tab 出去之后 AI 必须停手：SendInput 是系统级注入，
        // 不停就会往浏览器里敲 w、点左键。
        let mut a = agent();
        a.arm(true);
        a.ingest(sens(20.0, 2, 3), 0);
        a.input.set_foreground(false);
        let d = a.step(100, 5);
        assert_eq!(d.phase, Phase::Engage, "决策照常跑，只是不注入");
        assert!(!a.key_w && !a.btn_left, "后台时一个键都不许按");
        assert_eq!(a.bg_pauses, 1);
        assert!(a.paused_bg);
    }

    #[test]
    fn refocus_resumes_injection() {
        let mut a = agent();
        a.arm(true);
        a.ingest(sens(20.0, 2, 3), 0);
        a.input.set_foreground(false);
        a.step(100, 5);
        assert!(a.paused_bg);
        a.input.set_foreground(true);
        a.step(300, 5); // 前台状态 100ms 才刷一次，这里跨过去了
        assert!(!a.paused_bg, "回到游戏后应恢复");
        assert_eq!(a.bg_pauses, 1, "恢复不该再累加暂停次数");
    }

    #[test]
    fn pause_can_be_disabled() {
        let mut cfg = Config::default();
        cfg.pause_when_unfocused = false;
        let mut a = Agent::new(cfg, Box::new(DryInput::new()));
        a.arm(true);
        a.ingest(sens(20.0, 2, 3), 0);
        a.input.set_foreground(false);
        a.step(100, 5);
        assert_eq!(a.bg_pauses, 0, "关掉门控后就不该暂停");
    }

    // ---------------------------------------------------------- 开关反馈

    #[test]
    fn badge_tracks_armed_state() {
        let mut a = agent();
        a.update_badge();
        assert_eq!(a.input.last_badge().as_deref(), Some("[AI 待机]"));
        a.arm(true);
        a.update_badge();
        assert_eq!(a.input.last_badge().as_deref(), Some("[AI 接管]"));
        a.arm(false);
        a.update_badge();
        assert_eq!(a.input.last_badge().as_deref(), Some("[AI 待机]"));
    }

    #[test]
    fn arm_toggle_is_idempotent() {
        // 游戏内反复 `!ai on` 不该反复触发视角切换 —— arm() 靠这个判断
        let mut a = agent();
        assert!(!a.is_armed());
        a.arm(true);
        let ticks_before = a.machine.ticks;
        a.arm(true);
        assert!(a.is_armed());
        assert_eq!(a.machine.ticks, ticks_before);
    }

    // ---------------------------------------------------------- 战法 / 共同意识

    #[test]
    fn status_line_reports_strategy_and_squad() {
        let mut a = agent();
        let s = a.command("status", 0);
        assert!(s.contains("strat="), "状态行要看得到战法: {s}");
        assert!(s.contains("squad="), "状态行要看得到共同意识: {s}");
    }

    #[test]
    fn strat_command_toggles_one_strategy() {
        let mut a = agent();
        assert!(a.cfg.strategies.blitz);
        a.command("strat blitz", 0);
        assert!(!a.cfg.strategies.blitz);
        a.command("strat blitz", 0);
        assert!(a.cfg.strategies.blitz);
    }

    #[test]
    fn strat_off_disables_all_then_on_restores() {
        let mut a = agent();
        a.command("strat off", 0);
        let s = &a.cfg.strategies;
        assert!(!s.retreat && !s.wait && !s.blitz && !s.ambush && !s.feint);
        a.command("strat on", 0);
        let s = &a.cfg.strategies;
        assert!(s.retreat && s.wait && s.blitz && s.ambush && s.feint);
    }

    #[test]
    fn squad_on_off_keeps_manual_intent_consistent() {
        let mut a = agent();
        a.command("squad resume", 0); // 无效参数不该改状态
        assert_eq!(a.squad.manual, None);
        a.command("squad on", 0);
        assert!(a.squad.cfg.enabled);
        a.command("squad recall", 0);
        assert_eq!(a.squad.manual, Some(crate::squad::Intent::Recall));
        a.command("squad off", 0);
        assert!(!a.squad.cfg.enabled);
        assert_eq!(a.squad.manual, None, "关掉时要清掉手动意图，否则下次打开还是召回");
    }
}
