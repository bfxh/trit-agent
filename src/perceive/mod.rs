//! 感知层：把「游戏里发生了什么」翻译成事件流。
//!
//! 目前的唯一真实来源是客户端 `logs/latest.log`（Minecraft 把聊天、死亡、
//! 数据包 `tellraw` 的输出都写进去）。所以感知精度取决于游戏往聊天框里写了什么：
//! - 装 `trit_bridge` 数据包 → 每 0.5 秒一行 `<TRIT hp=... h=...>`，有血量、坐标、周围敌人数
//! - 不装 → 只知道死亡、聊天、以及你自己在聊天里打的指令

pub mod logtail;

#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    /// 0..=20
    pub hp: f32,
    pub food: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// 16 格内敌对实体数（数据包上限 24）
    pub hostiles: i32,
    /// 最近敌对生物的距离档（0 = 没有；1..33 = 该档下界格数）。
    /// 数据包算精确距离要遍历实体，这里只做几档半径判定，够用且便宜。
    pub dist_tier: i32,
    /// 最近爆炸源（苦力怕 / TNT / 床）的距离档，0 = 没有。
    pub hazard_tier: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogEvent {
    /// 玩家聊天
    Chat { player: String, text: String },
    /// 系统/服务器消息
    System(String),
    /// 数据包传感器行
    Sensor(Sensor),
    /// 自己死了
    Death(String),
    /// 进入世界
    Join(String),
    /// 离开世界
    Leave(String),
    /// `/data get` 风格的血量回显
    HealthProbe(f32),
}

const DEATH_MARKERS: &[&str] = &[
    "was slain by",
    "was shot by",
    "was blown up by",
    "was killed by",
    "was fireballed by",
    "was impaled by",
    "was struck by lightning",
    "was poked to death",
    "was pricked to death",
    "was stung to death",
    "was obliterated by",
    "was squashed by",
    "drowned",
    "died",
    "blew up",
    "fell from a high place",
    "fell too far",
    "hit the ground too hard",
    "burned to death",
    "went up in flames",
    "tried to swim in lava",
    "starved to death",
    "suffocated in a wall",
    "withered away",
    "froze to death",
    "experienced kinetic energy",
    "discovered the floor was lava",
    "left the confines of this world",
];

/// 把一行日志分类成事件。认不出来就返回 None —— 不猜。
pub fn classify(line: &str) -> Option<LogEvent> {
    // 传感器行优先（它可能被包在 [CHAT] 里）
    if let Some(idx) = line.find("<TRIT ") {
        if let Some(end) = line[idx..].find('>') {
            let body = &line[idx + 6..idx + end];
            if let Some(s) = parse_sensor(body) {
                return Some(LogEvent::Sensor(s));
            }
        }
    }

    let idx = line.find("[CHAT] ")?;
    let msg = line[idx + 7..].trim();

    // <玩家名> 消息
    if let Some(rest) = msg.strip_prefix('<') {
        if let Some(close) = rest.find('>') {
            let player = rest[..close].to_string();
            let text = rest[close + 1..].trim().to_string();
            return Some(LogEvent::Chat { player, text });
        }
    }

    if msg.contains("joined the game") {
        let name = msg.split_whitespace().next().unwrap_or("?").to_string();
        return Some(LogEvent::Join(name));
    }
    if msg.contains("left the game") {
        let name = msg.split_whitespace().next().unwrap_or("?").to_string();
        return Some(LogEvent::Leave(name));
    }

    if let Some(v) = msg.strip_prefix("has the following entity data: ") {
        if let Ok(f) = v.trim_end_matches('f').trim_end_matches('d').parse::<f32>() {
            return Some(LogEvent::HealthProbe(f));
        }
    }

    let lower = msg.to_ascii_lowercase();
    if DEATH_MARKERS.iter().any(|m| lower.contains(m)) {
        return Some(LogEvent::Death(msg.to_string()));
    }

    Some(LogEvent::System(msg.to_string()))
}

fn parse_sensor(body: &str) -> Option<Sensor> {
    // 形如：hp=200 food=17 x=123 y=-60 z=88 h=2 d=6 c=0
    // d / c 是后加的，老格式没有，缺失时按 0（= 没有该目标）处理。
    let mut hp = None;
    let mut food = 17;
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    let mut h = 0;
    let mut d = 0;
    let mut c = 0;
    for part in body.split_whitespace() {
        let (k, v) = part.split_once('=')?;
        match k {
            "hp" => hp = v.parse::<f32>().ok().map(|v| v / 10.0),
            "food" => food = v.parse().unwrap_or(17),
            "x" => x = v.parse::<f32>().map(|v| v / 10.0).unwrap_or(0.0),
            "y" => y = v.parse::<f32>().map(|v| v / 10.0).unwrap_or(0.0),
            "z" => z = v.parse::<f32>().map(|v| v / 10.0).unwrap_or(0.0),
            "h" => h = v.parse().unwrap_or(0),
            "d" => d = v.parse().unwrap_or(0),
            "c" => c = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    hp.map(|hp| Sensor {
        hp,
        food,
        x,
        y,
        z,
        hostiles: h,
        dist_tier: d,
        hazard_tier: c,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat() {
        let line = "[20:30:51] [Render thread/INFO]: [CHAT] <bfxh> hello there";
        match classify(line) {
            Some(LogEvent::Chat { player, text }) => {
                assert_eq!(player, "bfxh");
                assert_eq!(text, "hello there");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_sensor_line() {
        let line = "[20:30:51] [Render thread/INFO]: [CHAT] <TRIT hp=185 food=17 x=123 y=-601 z=88 h=3>";
        match classify(line) {
            Some(LogEvent::Sensor(s)) => {
                assert!((s.hp - 18.5).abs() < 0.01);
                assert_eq!(s.hostiles, 3);
                assert!((s.y - -60.1).abs() < 0.01);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_death() {
        let line = "[20:30:51] [Render thread/INFO]: [CHAT] bfxh was slain by Zombie";
        assert!(matches!(classify(line), Some(LogEvent::Death(_))));
    }

    #[test]
    fn parses_join() {
        let line = "[20:30:51] [Render thread/INFO]: [CHAT] bfxh joined the game";
        assert!(matches!(classify(line), Some(LogEvent::Join(_))));
    }

    #[test]
    fn unknown_line_is_system() {
        let line = "[20:30:51] [Render thread/INFO]: [CHAT] Press F3 to toggle debug";
        assert!(matches!(classify(line), Some(LogEvent::System(_))));
    }

    #[test]
    fn non_chat_lines_are_ignored() {
        let line = "[20:27:29] [Render thread/INFO]: Loaded 120 advancements";
        assert!(classify(line).is_none());
    }
}
