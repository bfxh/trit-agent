//! 把感知、状态机、输出三层接起来，跑「高频节拍」主循环。

use crate::act::{Button, Input};
use crate::behaviors::{Behaviors, Ctx};
use crate::config::{Config, Mode};
use crate::fsm::{Channel, Decision, Machine, Phase, Vote, WorldView};
use crate::perceive::logtail::LogTail;
use crate::perceive::{LogEvent, Sensor};
use crate::trit::Tri;
use std::io::Write;
use std::time::{Duration, Instant};

pub struct RunStats {
    pub ticks: u64,
    pub elapsed: Duration,
    pub achieved_hz: f64,
    pub attacks: u64,
    pub decisions: u64,
    pub phase_hist: [u64; 5],
    pub exits: u32,
}

pub struct Agent {
    pub cfg: Config,
    pub machine: Machine,
    pub behaviors: Behaviors,
    pub input: Box<dyn Input>,
    pub tail: Option<LogTail>,
    pub sensor: Option<Sensor>,

    /* 感知状态 */
    pub hp: Option<f32>,
    pub threat: Tri,
    pub threat_until: u64,
    pub contact_until: u64,
    pub dead: bool,

    /* 输出状态去重 */
    key_w: bool,
    key_s: bool,
    key_a: bool,
    key_d: bool,
    key_space: bool,
    key_shift: bool,
    btn_left: bool,
    btn_right: bool,

    /* 连续量做累加，避免每帧取整丢精度 */
    yaw_px: f32,
    pitch_px: f32,

    next_recover_ms: u64,
    next_log_ms: u64,
    armed: bool,
    channels: [bool; 7],
    run_log: Option<std::fs::File>,
    log_tick_stride: u64,
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
            hp: None,
            threat: Tri::Zero,
            threat_until: 0,
            contact_until: 0,
            dead: false,
            key_w: false,
            key_s: false,
            key_a: false,
            key_d: false,
            key_space: false,
            key_shift: false,
            btn_left: false,
            btn_right: false,
            yaw_px: 0.0,
            pitch_px: 0.0,
            next_recover_ms: 0,
            next_log_ms: 0,
            armed,
            channels,
            run_log: None,
            log_tick_stride: 20,
            cfg,
        }
    }

    pub fn attach_log(&mut self, tail: LogTail) {
        self.tail = Some(tail);
    }

    pub fn arm(&mut self, on: bool) {
        self.armed = on;
        self.machine.set_armed(on);
        if !on {
            self.release_all();
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed
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

    /// 处理一条游戏内事件。
    pub fn ingest(&mut self, ev: LogEvent, now_ms: u64) {
        match ev {
            LogEvent::Sensor(s) => {
                self.hp = Some(s.hp);
                self.dead = s.hp <= 0.0;
                if s.hostiles > 0 {
                    self.threat = Tri::Pos;
                    self.threat_until = now_ms + 2500;
                    self.contact_until = now_ms + 4000;
                } else {
                    self.threat = Tri::Neg;
                    self.threat_until = now_ms + 1200;
                }
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
            "status" => format!(
                "{} phase={} hp={}",
                if self.armed { "armed" } else { "off" },
                self.machine.phase.name(),
                self.hp.map(|h| format!("{h:.1}")).unwrap_or("?".into())
            ),
            _ => "? !ai !mode !attack !move !look !jump !use !threat !calm !status".to_string(),
        };
        reply
    }

    pub fn world(&self, tick: u64, now_ms: u64) -> WorldView {
        WorldView {
            hp: self.hp.map(|h| (h / 20.0).clamp(0.0, 1.0)),
            threat: self.threat,
            contact: self.contact_until > now_ms,
            stuck: false,
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
        let ctx = Ctx {
            w,
            phase: phase_now,
            now_ms,
            dt_ms,
        };
        let votes: Vec<Vote> = self.behaviors.vote(&ctx);
        let d = self.machine.step(&w, &votes, &self.channels);
        self.apply(&d, now_ms, dt_ms);
        d
    }

    fn apply(&mut self, d: &Decision, now_ms: u64, dt_ms: u64) {
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
        // 左键
        let want_attack = d.dirs[Channel::Attack.index()] == Tri::Pos;
        if want_attack != self.btn_left {
            self.btn_left = want_attack;
            self.input.button(Button::Left, want_attack);
        }
        // 右键
        let want_use = d.dirs[Channel::Use.index()] == Tri::Pos;
        if want_use != self.btn_right {
            self.btn_right = want_use;
            self.input.button(Button::Right, want_use);
        }
        // 跳
        self.set_key_space(d.dirs[Channel::Jump.index()] == Tri::Pos);
        // 疾跑：前进时按住 Shift
        let running = self.cfg.sprint && d.dirs[Channel::Forward.index()] == Tri::Pos;
        if running != self.key_shift {
            self.key_shift = running;
            self.input.key(crate::act::vk::SHIFT, running);
        }
        // 死亡：点重生
        if d.recover_click && now_ms >= self.next_recover_ms {
            self.next_recover_ms = now_ms + self.cfg.respawn_click_ms;
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

        // 启动时先切第三视角
        if self.cfg.third_person_on_start {
            self.input.focus();
            std::thread::sleep(Duration::from_millis(120));
            for _ in 0..self.cfg.perspective_presses {
                self.input.key(crate::act::vk::F5, true);
                std::thread::sleep(Duration::from_millis(60));
                self.input.key(crate::act::vk::F5, false);
                std::thread::sleep(Duration::from_millis(60));
            }
        }

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
                    }
                }
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
            let elapsed = now.duration_since(start);
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
        let elapsed = start.elapsed();
        RunStats {
            ticks,
            elapsed,
            achieved_hz: ticks as f64 / elapsed.as_secs_f64().max(1e-9),
            attacks,
            decisions: ticks,
            phase_hist,
            exits: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            LogEvent::Sensor(Sensor {
                hp: 20.0,
                food: 17,
                x: 0.0,
                y: 64.0,
                z: 0.0,
                hostiles: 3,
            }),
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
    fn panic_releases_everything() {
        let mut a = agent();
        a.arm(true);
        a.ingest(
            LogEvent::Sensor(Sensor {
                hp: 20.0,
                food: 17,
                x: 0.0,
                y: 64.0,
                z: 0.0,
                hostiles: 1,
            }),
            0,
        );
        a.step(50, 5);
        a.command("panic", 60);
        assert!(!a.is_armed());
        assert!(!a.key_w && !a.key_s && !a.btn_left);
    }
}
