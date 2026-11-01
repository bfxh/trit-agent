//! 端到端：日志文本 → 解析 → 状态机 → 输入动作。
//! 这一层不碰硬件，所以 CI 和没装游戏的机器都能跑。

use std::io::Write;
use trit_agent::act::dry::DryInput;
use trit_agent::agent::Agent;
use trit_agent::config::Config;
use trit_agent::fsm::{Channel, Phase};
use trit_agent::perceive::LogEvent;
use trit_agent::perceive::logtail::LogTail;

struct Harness {
    agent: Agent,
    tail: LogTail,
    path: std::path::PathBuf,
    t: u64,
}

impl Harness {
    fn new(tag: &str) -> Harness {
        let dir = std::env::temp_dir().join(format!("trit-agent-it-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("latest.log");
        std::fs::write(&path, "").unwrap();

        let cfg = Config::from_str("assume_threat = false\nhz = 200\n");
        let mut agent = Agent::new(cfg, Box::new(DryInput::new()));
        agent.arm(true);
        let mut tail = LogTail::new(&path);
        tail.skip_to_end();
        Harness {
            agent,
            tail,
            path,
            t: 0,
        }
    }

    fn push(&mut self, line: &str) {
        let mut f = std::fs::OpenOptions::new().append(true).open(&self.path).unwrap();
        writeln!(f, "[21:00:00] [Render thread/INFO]: {line}").unwrap();
        drop(f);
        let events = self.tail.poll();
        assert!(!events.is_empty(), "日志行没被识别: {line}");
        for ev in events {
            self.agent.ingest(ev, self.t);
        }
        self.t += 100;
    }

    fn tick(&mut self) -> trit_agent::fsm::Decision {
        let d = self.agent.step(self.t, 5);
        self.t += 5;
        d
    }
}

#[test]
fn sensor_line_drives_engage_and_attack() {
    let mut h = Harness::new("engage");
    let d = h.tick();
    assert_eq!(d.phase, Phase::Idle, "没数据应该站着不动");

    h.push("[CHAT] <TRIT hp=200 food=17 x=0 y=640 z=0 h=3>");
    let d = h.tick();
    assert_eq!(d.phase, Phase::Engage);
    assert_eq!(d.dirs[Channel::Attack.index()], trit_agent::trit::Tri::Pos);
}

#[test]
fn low_hp_retreats_then_death_recovers() {
    let mut h = Harness::new("retreat");
    h.push("[CHAT] <TRIT hp=200 food=17 x=0 y=640 z=0 h=2>");
    assert_eq!(h.tick().phase, Phase::Engage);

    h.push("[CHAT] <TRIT hp=40 food=17 x=0 y=640 z=0 h=2>");
    let d = h.tick();
    assert_eq!(d.phase, Phase::Retreat);
    assert_eq!(d.dirs[Channel::Forward.index()], trit_agent::trit::Tri::Neg);

    h.push("[CHAT] bfxh was slain by Zombie");
    let d = h.tick();
    assert_eq!(d.phase, Phase::Recover);
    assert!(d.recover_click);

    // 重生后传感器报平安 → 回到待命
    h.push("[CHAT] <TRIT hp=200 food=20 x=8 y=640 z=8 h=0>");
    let d = h.tick();
    assert_eq!(d.phase, Phase::Idle);
}

#[test]
fn clear_threat_lets_regen_stand_still() {
    let mut h = Harness::new("regen");
    // 低血、无敌情：Regen 的弃权票应该把巡逻的前进压掉
    let cfg = Config::from_str("assume_threat = false\nmode = patrol\n");
    h.agent.cfg = cfg;
    h.agent.behaviors = trit_agent::behaviors::Behaviors::new(&h.agent.cfg);
    h.push("[CHAT] <TRIT hp=60 food=17 x=0 y=640 z=0 h=0>");
    let d = h.tick();
    assert_eq!(d.phase, Phase::Idle);
    assert_eq!(d.dirs[Channel::Forward.index()], trit_agent::trit::Tri::Zero);
}

#[test]
fn chat_command_can_take_control_back() {
    let mut h = Harness::new("chat");
    h.push("[CHAT] <TRIT hp=200 food=17 x=0 y=640 z=0 h=2>");
    assert!(h.agent.is_armed());
    h.push("[CHAT] <bfxh> !ai off");
    assert!(!h.agent.is_armed());
    let d = h.tick();
    assert_eq!(d.phase, Phase::Off);
    for ch in Channel::all() {
        assert_eq!(d.dirs[ch.index()], trit_agent::trit::Tri::Zero);
    }
}

#[test]
fn death_event_is_classified_not_misfiled_as_chat() {
    let ev = trit_agent::perceive::classify("[x] [Render thread/INFO]: [CHAT] bfxh blew up");
    assert!(matches!(ev, Some(LogEvent::Death(_))));
}
