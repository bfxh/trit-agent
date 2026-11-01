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
    /// 最近原木的距离档，0 = 8 方向 × 3 档全没命中（视野内没木头）。
    ///
    /// 数据包沿「当前朝向 ±45° 的 8 个方向」做射线判定，取最近。
    /// 分辨率是方位 45°、距离 3 档 —— 只够「朝那边走」，不够瞄准。
    pub wood_tier: i32,
    /// 木头在哪个方向：0 正前，负左正右，单位 45°（-3..=3）。
    pub wood_dir: i32,
    /// 游戏内时间 0..=23999。-1 = 没读到。
    pub tod: i32,
    /// 视线俯仰角，度。负 = 往下看，正 = 往上看，0 = 平视。
    ///
    /// 这一维很关键：三值状态机只说得清"往哪转"，说不清"转到多少度"。
    /// 没有绝对值，砍树就是开环瞎转 —— 转多了准星跑到脚面，转少了对不上树干。
    pub pitch: f32,
}

impl Sensor {
    /// 是否夜晚（原版 20 分钟一天：0..12000 白天，12000.. 开始暗）。
    ///
    /// 取 13000 作边界：12542 是怪物能开始刷的时刻，留一点余量。
    pub fn is_night(&self) -> Option<bool> {
        if self.tod < 0 {
            return None;
        }
        Some(self.tod >= 13_000 && self.tod < 23_000)
    }
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
    // 形如：hp=200 food=17 x=123 y=-60 z=88 h=2 d=6 c=0 w=6 wd=-1 t=15200
    // 后加的字段（d / c / w / wd / t）老格式没有，缺失时按「没有该目标」处理。
    let mut hp = None;
    let mut food = 17;
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    let mut h = 0;
    let mut d = 0;
    let mut c = 0;
    let mut w = 0;
    let mut wd = 0;
    let mut tod = -1;
    let mut pitch = 0.0;
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
            "w" => w = v.parse().unwrap_or(0),
            "wd" => wd = v.parse().unwrap_or(0),
            "t" => tod = v.parse().unwrap_or(-1),
            "p" => pitch = v.parse::<f32>().map(|v| v / 10.0).unwrap_or(0.0),
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
        wood_tier: w,
        wood_dir: wd,
        tod,
        pitch,
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

    #[test]
    fn parses_wood_and_daytime() {
        let line = "[20:30:51] [Render thread/INFO]: [CHAT] <TRIT hp=200 food=18 x=1 y=64 z=1 h=0 d=0 c=0 w=6 wd=-1 t=15200>";
        match classify(line) {
            Some(LogEvent::Sensor(s)) => {
                assert_eq!(s.wood_tier, 6);
                assert_eq!(s.wood_dir, -1);
                assert_eq!(s.tod, 15_200);
                assert_eq!(s.is_night(), Some(true));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn old_sensor_line_still_parses() {
        // 老版本 trit_bridge 输出的行没有 w/wd/t，必须照常能用 —— 否则升级
        // Rust 侧会要求用户同步重新装数据包，而这种耦合应该反过来。
        let line = "[x] [Render thread/INFO]: [CHAT] <TRIT hp=180 food=17 x=0 y=64 z=0 h=1 d=3 c=0>";
        match classify(line) {
            Some(LogEvent::Sensor(s)) => {
                assert_eq!(s.wood_tier, 0);
                assert_eq!(s.tod, -1, "读不到时间就该是未知，不能当白天");
                assert_eq!(s.is_night(), None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn night_boundaries() {
        let mut s = Sensor {
            hp: 20.0,
            food: 20,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            hostiles: 0,
            dist_tier: 0,
            hazard_tier: 0,
            wood_tier: 0,
            wood_dir: 0,
            tod: 0,
            pitch: 0.0,
        };
        assert_eq!(s.is_night(), Some(false)); // 日出
        s.tod = 12_000;
        assert_eq!(s.is_night(), Some(false), "正午刚过，天还亮着");
        s.tod = 13_000;
        assert_eq!(s.is_night(), Some(true));
        s.tod = 23_000;
        assert_eq!(s.is_night(), Some(false), "天亮了该干活");
        s.tod = -1;
        assert_eq!(s.is_night(), None);
    }
}
