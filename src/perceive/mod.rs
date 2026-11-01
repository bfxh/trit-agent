//! 感知层：把「游戏里发生了什么」翻译成事件流。
//!
//! 目前的唯一真实来源是客户端 `logs/latest.log`（Minecraft 把聊天、死亡、
//! 数据包 `tellraw` 的输出都写进去）。所以感知精度取决于游戏往聊天框里写了什么：
//! - 装 `trit_bridge` 数据包 → 慢通道每 1 秒一行 `<TRIT ...>`、快通道每 100ms 一行 `<TRITF ...>`
//! - 不装 → 只知道死亡、聊天、以及你自己在聊天里打的指令
//!
//! ## 两条通道为什么用「补丁」而不是两个结构
//!
//! 两条通道报的是**同一个世界的不同切片**：快通道报瞄准角和碰撞，慢通道报
//! 敌人和资源方位。如果各存一份，行为层每次取数都要问「该信哪一份、这份还新不新」，
//! 而正确答案是「各自最新的那份」—— 于是就有了 [`SensorPatch`]：
//! 每条帧只带自己有的字段，[`Sensor::apply`] 把有的覆盖上去，没的就留着。
//!
//! 这样上层只面对一个 [`Sensor`]，不用知道某条数据是从哪条通道来的。

pub mod logtail;

use std::fmt;

/// 方块类别。**编码必须和 `scripts/make_trit_bridge.py` 里的 `CLS_*` 一致**。
///
/// 分类的标准不是「它是什么方块」，而是「拿它怎么办」：
/// 能不能过去、值不值得挖、挖了会不会死。同一类方块在这个视角下没有区别，
/// 所以一个 u8 就够 —— 这正是一条 tellraw 能塞下十几个判定结果的原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Block {
    /// 空气 / 能穿过去的植物
    #[default]
    Empty = 0,
    /// 原木 —— 砍
    Log = 1,
    /// 矿石 —— 挖
    Ore = 2,
    /// 固体 —— 挡路，或者挖了不值钱
    Solid = 3,
    /// 水 —— 能游，但别在这儿挖
    Water = 4,
    /// 岩浆 —— 绝对不挖
    Lava = 5,
    /// 火 / 仙人掌 / 甜浆果 / 岩浆块
    Hazard = 6,
}

impl Block {
    pub fn from_code(code: i32) -> Option<Block> {
        Some(match code {
            0 => Block::Empty,
            1 => Block::Log,
            2 => Block::Ore,
            3 => Block::Solid,
            4 => Block::Water,
            5 => Block::Lava,
            6 => Block::Hazard,
            // 未来加了新类别而 Rust 还没跟上时，宁可「不知道」也不要猜成固体：
            // 猜成固体会让 AI 撞墙，猜成空会让它走进岩浆，两边都是错的。
            _ => return None,
        })
    }

    /// 人能不能直接穿过去
    pub fn passable(self) -> bool {
        matches!(self, Block::Empty | Block::Water)
    }

    /// 站在上面会不会受伤 / 掉下去
    pub fn dangerous_floor(self) -> bool {
        matches!(self, Block::Lava | Block::Hazard | Block::Empty)
    }

    /// 挖它有没有好处
    pub fn rewarding(self) -> bool {
        matches!(self, Block::Log | Block::Ore)
    }

    pub fn name(self) -> &'static str {
        match self {
            Block::Empty => "air",
            Block::Log => "log",
            Block::Ore => "ore",
            Block::Solid => "solid",
            Block::Water => "water",
            Block::Lava => "lava",
            Block::Hazard => "danger",
        }
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 八方位墙体掩码里，某个方位是否被挡住。
///
/// `dir` 的语义跟 `wood_dir` 一致：`0` 正前，负左正右，单位 45°。
///
/// 位序是「相对朝向 yaw 偏移 +i×45°」，而 MC 里 **yaw 增加是往右转**
/// （0=南，90=西，从南转到西是顺时针，也就是右手边）。所以
/// `bit = dir`，左边倒着数回去：`dir = -1` 落到位 7。
///
/// 这个映射第一次写反过，而且当时配套的测试也照错的假设写，所以全绿 ——
/// 表现只会在「往右边绕墙却撞上左边的墙」时出现，极难追。改这里的位序前
/// 先想清楚 yaw 到底往哪边加。
pub fn wall_at(wall: u8, dir: i32) -> bool {
    let bit = dir.rem_euclid(8) as u32;
    wall & (1 << bit) != 0
}

/// 一整帧状态（两条通道合并后的结果）。
#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    /// 0..=20
    pub hp: f32,
    pub food: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// 32 格内敌对实体数（数据包上限 32）
    pub hostiles: i32,
    /// 最近敌对生物的距离档（0 = 没有；其余是该档下界格数）。
    /// 数据包算精确距离要遍历实体，这里只做几档半径判定，够用且便宜。
    pub dist_tier: i32,
    /// 最近爆炸源（苦力怕 / TNT / 床）的距离档，0 = 没有。
    pub hazard_tier: i32,
    /// 最近原木的距离档，0 = 8 方向 × 3 档全没命中（视野内没木头）。
    pub wood_tier: i32,
    /// 木头在哪个方向：0 正前，负左正右，单位 45°（-3..=3）。
    pub wood_dir: i32,
    /// 最近矿石的距离档，0 = 视野内没矿。
    pub ore_tier: i32,
    pub ore_dir: i32,
    /// 游戏内时间 0..=23999。-1 = 没读到。
    pub tod: i32,
    /// 视线俯仰角，度。负 = 往下看，正 = 往上看，0 = 平视。
    ///
    /// 这一维很关键：三值状态机只说得清"往哪转"，说不清"转到多少度"。
    /// 没有绝对值，砍树就是开环瞎转 —— 转多了准星跑到脚面，转少了对不上树干。
    pub pitch: f32,
    /// 八方位墙体掩码，见 [`wall_at`]。`None` = 快通道没开，不知道有没有墙。
    pub wall: Option<u8>,
    /// 准星正对的方块（沿视线，含俯仰）—— 挖矿的目标就是它。
    pub blk_front: Option<Block>,
    /// 脚下踩着的方块。挖矿最怕的就是把这块挖了。
    pub blk_below: Option<Block>,
    /// 正前方是「一格高的台阶」吗？前面有固体、它上面没有。
    ///
    /// 单独一维而不是从 `wall` 推：掩码只说"1 格处有固体"，一格土坎和两格墙
    /// 在里面长得一模一样。少了这一维，AI 会把每一个土坎都当死路 ——
    /// 表现出来是「明明能走的地方它站住不动」，而且只在有地形起伏的地方发作。
    pub step: Option<bool>,
}

impl Default for Sensor {
    fn default() -> Self {
        Sensor {
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
            ore_tier: 0,
            ore_dir: 0,
            tod: -1,
            pitch: 0.0,
            wall: None,
            blk_front: None,
            blk_below: None,
            step: None,
        }
    }
}

/// 一条帧里带了哪些字段。缺的字段是 `None` —— 「这条没报」，不是「值是零」。
///
/// 这个区分是整套感知的关键：慢通道不报墙体，如果默认成 `wall: 0`（没有墙），
/// 那么「没数据」和「确认没墙」就分不开了，AI 会在地面上放心撞墙。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SensorPatch {
    pub hp: Option<f32>,
    pub food: Option<i32>,
    pub pos: Option<(f32, f32, f32)>,
    pub hostiles: Option<i32>,
    pub dist_tier: Option<i32>,
    pub hazard_tier: Option<i32>,
    pub wood_tier: Option<i32>,
    pub wood_dir: Option<i32>,
    pub ore_tier: Option<i32>,
    pub ore_dir: Option<i32>,
    pub tod: Option<i32>,
    pub pitch: Option<f32>,
    pub wall: Option<u8>,
    pub blk_front: Option<Block>,
    pub blk_below: Option<Block>,
    pub step: Option<bool>,
    /// 这一帧来自快通道吗（决定延迟统计口径）
    pub fast: bool,
}

impl Sensor {
    /// 把一条帧的字段覆盖上来。没报的字段保持原值。
    pub fn apply(&mut self, p: &SensorPatch) {
        if let Some(v) = p.hp {
            self.hp = v;
        }
        if let Some(v) = p.food {
            self.food = v;
        }
        if let Some((x, y, z)) = p.pos {
            self.x = x;
            self.y = y;
            self.z = z;
        }
        if let Some(v) = p.hostiles {
            self.hostiles = v;
        }
        if let Some(v) = p.dist_tier {
            self.dist_tier = v;
        }
        if let Some(v) = p.hazard_tier {
            self.hazard_tier = v;
        }
        if let Some(v) = p.wood_tier {
            self.wood_tier = v;
        }
        if let Some(v) = p.wood_dir {
            self.wood_dir = v;
        }
        if let Some(v) = p.ore_tier {
            self.ore_tier = v;
        }
        if let Some(v) = p.ore_dir {
            self.ore_dir = v;
        }
        if let Some(v) = p.tod {
            self.tod = v;
        }
        if let Some(v) = p.pitch {
            self.pitch = v;
        }
        if let Some(v) = p.wall {
            self.wall = Some(v);
        }
        if let Some(v) = p.blk_front {
            self.blk_front = Some(v);
        }
        if let Some(v) = p.blk_below {
            self.blk_below = Some(v);
        }
        if let Some(v) = p.step {
            self.step = Some(v);
        }
    }

    /// 是否夜晚（原版 20 分钟一天：0..12000 白天，12000.. 开始暗）。
    ///
    /// 取 13000 作边界：12542 是怪物能开始刷的时刻，留一点余量。
    pub fn is_night(&self) -> Option<bool> {
        if self.tod < 0 {
            return None;
        }
        Some(self.tod >= 13_000 && self.tod < 23_000)
    }

    /// 正前方 1 格有没有东西挡着。
    pub fn blocked_ahead(&self) -> Option<bool> {
        self.wall.map(|w| wall_at(w, 0))
    }

    /// 往某个方向走会不会撞墙。`None` = 不知道（快通道没开）。
    pub fn blocked(&self, dir: i32) -> Option<bool> {
        self.wall.map(|w| wall_at(w, dir))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogEvent {
    /// 玩家聊天
    Chat { player: String, text: String },
    /// 系统/服务器消息
    System(String),
    /// 数据包传感器帧（快慢两条通道都是这个）
    Sensor(SensorPatch),
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
///
/// **这个函数每秒被调几十到几百次**（Forge 日志什么都会往里写），所以
/// 下面这行 `find("[CHAT] ")` 的早退不是微优化，是必需的：没有它，每一行
/// 无关日志都要跑一遍 26 个死亡标记的 `contains`。实测开快通道后日志量翻十倍，
/// 那 26 次搜索就是白白烧掉的 CPU —— 而它们只对聊天行有意义。
pub fn classify(line: &str) -> Option<LogEvent> {
    // 传感器行优先（它也被包在 [CHAT] 里）
    if let Some(idx) = line.find("<TRIT") {
        if let Some(end) = line[idx..].find('>') {
            let body = &line[idx + 5..idx + end];
            let (tag, body) = match body.split_once(' ') {
                Some((t, b)) => (t, b),
                None => (body, ""),
            };
            if tag == "F" {
                if let Some(p) = parse_patch(body, true) {
                    return Some(LogEvent::Sensor(p));
                }
            } else if let Some(p) = parse_patch(body, false) {
                return Some(LogEvent::Sensor(p));
            }
        }
    }

    // 早退：不是聊天行就到此为止。mod 的日志、启动信息、堆栈都走这条路。
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

/// 解析一帧。`fast` 决定哪些字段算「这条通道会报的」。
///
/// 两条通道共用一套 key=value 语法，只有快通道带 `wal` / `b1` / `bd`。
/// 老版本数据包没有 `w` / `wd` / `t` / `p` 这些字段，缺失时按 `None` 处理，
/// 于是「升级 Rust 就得同步重装数据包」这种耦合不会出现。
fn parse_patch(body: &str, fast: bool) -> Option<SensorPatch> {
    // 慢通道形如：hp=200 food=17 x=123 y=-60 z=88 h=2 d=6 c=0 w=6 wd=-1 o=4 od=1 t=15200 p=150
    // 快通道形如：wal=5 b1=2 bd=3 p=220
    let mut p = SensorPatch {
        fast,
        ..Default::default()
    };
    let mut hp_seen = false;
    for part in body.split_whitespace() {
        let (k, v) = match part.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match k {
            "hp" => {
                if let Ok(x) = v.parse::<f32>() {
                    p.hp = Some(x / 10.0);
                    hp_seen = true;
                }
            }
            "food" => p.food = v.parse().ok(),
            "x" => p.pos = v.parse::<f32>().ok().map(|x| x / 10.0).map(|x| (x, 0.0, 0.0)),
            "y" => {
                if let Ok(y) = v.parse::<f32>() {
                    let (px, _, pz) = p.pos.unwrap_or((0.0, 0.0, 0.0));
                    p.pos = Some((px, y / 10.0, pz));
                }
            }
            "z" => {
                if let Ok(z) = v.parse::<f32>() {
                    let (px, py, _) = p.pos.unwrap_or((0.0, 0.0, 0.0));
                    p.pos = Some((px, py, z / 10.0));
                }
            }
            "h" => p.hostiles = v.parse().ok(),
            "d" => p.dist_tier = v.parse().ok(),
            "c" => p.hazard_tier = v.parse().ok(),
            "w" => p.wood_tier = v.parse().ok(),
            "wd" => p.wood_dir = v.parse().ok(),
            "o" => p.ore_tier = v.parse().ok(),
            "od" => p.ore_dir = v.parse().ok(),
            "t" => p.tod = v.parse().ok(),
            "p" => p.pitch = v.parse::<f32>().ok().map(|x| x / 10.0),
            "wal" => p.wall = v.parse::<i32>().ok().map(|x| x.clamp(0, 255) as u8),
            "b1" => p.blk_front = v.parse::<i32>().ok().and_then(Block::from_code),
            "bd" => p.blk_below = v.parse::<i32>().ok().and_then(Block::from_code),
            "s" => p.step = v.parse::<i32>().ok().map(|x| x != 0),
            _ => {}
        }
    }
    if fast {
        // 快通道不含血量，但它如果一条有效字段都没有，就说明这不是一帧
        if p.wall.is_none() && p.blk_front.is_none() && p.blk_below.is_none() && p.step.is_none()
        {
            return None;
        }
        return Some(p);
    }
    // 慢通道必须有血量，否则不是传感器行
    if !hp_seen {
        return None;
    }
    Some(p)
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
                assert!((s.hp.unwrap() - 18.5).abs() < 0.01);
                assert_eq!(s.hostiles, Some(3));
                assert!((s.pos.unwrap().1 - -60.1).abs() < 0.01);
                assert!(!s.fast);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_fast_frame() {
        let line = "[x] [Render thread/INFO]: [CHAT] <TRITF wal=5 b1=2 bd=3 p=350>";
        match classify(line) {
            Some(LogEvent::Sensor(p)) => {
                assert!(p.fast);
                assert_eq!(p.wall, Some(5));
                assert_eq!(p.blk_front, Some(Block::Ore));
                assert_eq!(p.blk_below, Some(Block::Solid));
                assert!((p.pitch.unwrap() - 35.0).abs() < 0.01);
                assert_eq!(p.hp, None, "快通道不该带血量");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fast_frame_does_not_wipe_slow_fields() {
        // 这是整个补丁模型的立身之本：快通道没报的字段必须保持原值，
        // 否则每 100ms 就会把"有几个怪"清成 0，AI 会以为世界永远安全。
        let mut s = Sensor::default();
        s.apply(&parse_patch("hp=200 food=17 h=3 d=6 w=6 wd=-1 t=8000", false).unwrap());
        s.apply(&parse_patch("wal=1 b1=3 bd=3 p=200", true).unwrap());
        assert_eq!(s.hostiles, 3, "快帧不能清掉敌人数");
        assert_eq!(s.food, 17);
        assert_eq!(s.wood_tier, 6);
        assert_eq!(s.wall, Some(1));
        assert!((s.pitch - 20.0).abs() < 0.01);
    }

    #[test]
    fn old_fast_frame_without_wall_is_rejected() {
        // 快通道如果一条有效字段都没有，不能当成"世界是空的"
        assert!(parse_patch("", true).is_none());
        assert!(parse_patch("nonsense", true).is_none());
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
        // 含死亡关键词但不是聊天行的也不能误判
        let line = "[20:27:29] [Render thread/INFO]: Recipe died loaded";
        assert!(classify(line).is_none());
    }

    #[test]
    fn parses_wood_ore_and_daytime() {
        let line = "[x] [Render thread/INFO]: [CHAT] <TRIT hp=200 food=18 x=1 y=64 z=1 h=0 d=0 c=0 w=6 wd=-1 o=4 od=1 t=15200 p=-50>";
        match classify(line) {
            Some(LogEvent::Sensor(s)) => {
                assert_eq!(s.wood_tier, Some(6));
                assert_eq!(s.wood_dir, Some(-1));
                assert_eq!(s.ore_tier, Some(4));
                assert_eq!(s.ore_dir, Some(1));
                assert_eq!(s.tod, Some(15_200));
                assert!((s.pitch.unwrap() - -5.0).abs() < 0.01);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn old_sensor_line_still_parses() {
        // 老版本数据包输出的行没有 w/wd/t/o，必须照常能用
        let line = "[x] [Render thread/INFO]: [CHAT] <TRIT hp=180 food=17 x=0 y=64 z=0 h=1 d=3 c=0>";
        match classify(line) {
            Some(LogEvent::Sensor(s)) => {
                assert_eq!(s.wood_tier, None);
                assert_eq!(s.tod, None, "读不到时间就该是未知，不能当白天");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn night_boundaries() {
        let mut s = Sensor::default();
        s.tod = 0;
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

    #[test]
    fn wall_mask_maps_directions_correctly() {
        // 位 i = 相对朝向 yaw 偏移 +i×45°，而 MC 的 yaw 增加是**往右转**
        // （0=南、90=西；从南到西是顺时针 = 右手边）。所以正右方 = 位 1，
        // 正左方 = 位 7。这个方向感写反过一次，配套的测试也照错的写，全绿。
        let only_front = 1u8 << 0;
        assert!(wall_at(only_front, 0));
        assert!(!wall_at(only_front, 1));
        assert!(!wall_at(only_front, -1));

        let only_right = 1u8 << 1;
        assert!(wall_at(only_right, 1), "yaw +45 是右手边");
        assert!(!wall_at(only_right, -1), "别把右边认成左边");

        let only_left = 1u8 << 7;
        assert!(wall_at(only_left, -1), "yaw -45 是左手边");
        assert!(!wall_at(only_left, 1));

        // 背后
        assert!(wall_at(1u8 << 4, 4));
        assert!(wall_at(1u8 << 4, -4), "绕一圈回来还是背后");
        assert!(!wall_at(1u8 << 4, 0));

        // 全方向都有墙
        assert!(wall_at(0xff, 3));
        assert!(wall_at(0xff, -3));
        // 没有墙
        assert!(!wall_at(0, 0));
    }

    #[test]
    fn block_codes_round_trip() {
        for code in 0..=6 {
            let b = Block::from_code(code).unwrap();
            assert_eq!(b as u8 as i32, code);
        }
        assert_eq!(Block::from_code(99), None, "未知类别不能猜");
        assert!(Block::Empty.passable());
        assert!(Block::Water.passable());
        assert!(!Block::Solid.passable());
        assert!(Block::Lava.dangerous_floor());
        assert!(Block::Empty.dangerous_floor(), "脚下是空就是悬空");
        assert!(!Block::Solid.dangerous_floor());
        assert!(Block::Ore.rewarding());
        assert!(!Block::Solid.rewarding());
    }

    #[test]
    fn no_wall_data_means_unknown_not_clear() {
        // 快通道没开时 wall 是 None，「不知道」不能当成「没墙」
        let s = Sensor::default();
        assert_eq!(s.blocked(0), None);
        assert_eq!(s.blocked_ahead(), None);
    }
}
