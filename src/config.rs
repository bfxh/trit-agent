//! 配置：极简 `key = value`，不引入 toml 依赖。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// 站桩护主：原地小幅扫视，按节奏挥击。
    Hold,
    /// 巡逻：一直前进，周期性扫视，卡住就跳。
    Patrol,
    /// 跟随：前进 + 定期跳，适合跟着队伍跑。
    Follow,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "patrol" | "巡逻" => Mode::Patrol,
            "follow" | "跟随" => Mode::Follow,
            _ => Mode::Hold,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Hold => "hold",
            Mode::Patrol => "patrol",
            Mode::Follow => "follow",
        }
    }
}

/// 「次操作」开关。每一项都能单独关掉，关掉后该通道永久弃权（Zero），
/// 把控制权完整交还给你。
#[derive(Clone, Copy, Debug)]
pub struct Channels {
    pub movement: bool,
    pub look: bool,
    pub attack: bool,
    pub jump: bool,
    pub use_item: bool,
}

impl Default for Channels {
    fn default() -> Self {
        Channels {
            movement: true,
            look: true,
            attack: true,
            jump: true,
            use_item: true,
        }
    }
}

/// 战术开关。四条全部移植自整合包里 mob mod 的 Goal，能单独关。
///
/// 关掉任一条 = 恢复「没有这一档战术」的原始行为，方便定位是谁把事情搞砸。
#[derive(Clone, Copy, Debug)]
pub struct Tactics {
    /// 交战距离带：太远追、刚好站住、贴脸退半步。
    /// 原版对应 `MeleeAttackGoal`（3 格）+ `EARangedBowAttackGoal`（7 格）。
    pub range: bool,
    /// 爆炸物规避：苦力怕 10 格、TNT 3 格。对应 `AvoidCreeperGoal`/`AvoidTNTGoal`。
    pub hazard: bool,
    /// 受伤且近身时按右键格挡。对应 `ShieldingGoal` / `RaiseShieldGoal`。
    /// 注意：手上拿的是食物时右键会触发进食，不想要就关掉这条。
    pub guard: bool,
    /// 卡住 1.2 秒就低头砍墙。对应 `BlockBreakGoal` / `DiggingGoal`。
    pub breach: bool,
}

impl Default for Tactics {
    fn default() -> Self {
        Tactics {
            range: true,
            hazard: true,
            guard: true,
            breach: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// 状态机内部节拍。默认 200 Hz（游戏是 20 TPS，这里是它的 10 倍，
    /// 输入抖动被内部高频抹平）。
    pub hz: u32,
    pub window_title: String,
    /// 启动即切第三视角（注入 F5）。
    pub third_person_on_start: bool,
    pub perspective_presses: u8,

    pub kill_switch_vk: u16,
    pub panic_vk: u16,
    pub toggle_attack_vk: u16,
    pub toggle_move_vk: u16,

    pub mode: Mode,
    pub channels: Channels,
    pub tactics: Tactics,

    pub attack_interval_ms: u64,
    pub jump_crit_every: u32,
    pub strafe_flip_ms: u64,
    /// 视角转速，度/秒。三值状态机只决定「往哪转」，转速是连续量，单独给。
    pub look_rate: f32,
    pub low_hp: f32,
    pub patrol_jump_ms: u64,
    pub respawn_click_ms: u64,

    /// 鼠标灵敏度换算：每个决策周期转的角度 * look_gain = 鼠标像素。
    pub look_gain: f32,
    pub invert_pitch: bool,

    pub log_path: PathBuf,
    pub run_log: PathBuf,
    /// 是否需要额外按下 Shift 疾跑。
    pub sprint: bool,
    /// 没有传感器数据时，是否允许「无脑打架」。
    /// 默认 false：没感知就站桩，这是「Unknown 不猜」的直接体现。
    pub assume_threat: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hz: 200,
            window_title: "Minecraft".to_string(),
            third_person_on_start: true,
            perspective_presses: 1,
            kill_switch_vk: 0x77, // F8
            panic_vk: 0x78,       // F9
            toggle_attack_vk: 0x75, // F6
            toggle_move_vk: 0x76,   // F7
            mode: Mode::Hold,
            channels: Channels::default(),
            tactics: Tactics::default(),
            attack_interval_ms: 620,
            jump_crit_every: 2,
            strafe_flip_ms: 900,
            look_rate: 55.0,
            low_hp: 0.35,
            patrol_jump_ms: 2600,
            respawn_click_ms: 1500,
            look_gain: 1.35,
            invert_pitch: false,
            log_path: PathBuf::from(""),
            run_log: PathBuf::from("trit-agent-run.log"),
            sprint: true,
            assume_threat: false,
        }
    }
}

impl Config {
    /// 不存在就写一份默认配置出来，然后读取。
    pub fn load_or_create(path: &Path) -> std::io::Result<Config> {
        if !path.exists() {
            fs::write(path, DEFAULT_TOML)?;
        }
        let text = fs::read_to_string(path)?;
        Ok(Config::from_str(&text))
    }

    pub fn from_str(text: &str) -> Config {
        let mut c = Config::default();
        let map = parse_kv(text);
        let g = |k: &str| map.get(k).cloned();

        if let Some(v) = g("hz").and_then(|v| v.parse::<u32>().ok()) {
            c.hz = v.clamp(1, 2000);
        }
        if let Some(v) = g("window_title") {
            c.window_title = v;
        }
        if let Some(v) = g("third_person_on_start").and_then(|v| parse_bool(&v)) {
            c.third_person_on_start = v;
        }
        if let Some(v) = g("perspective_presses").and_then(|v| v.parse::<u8>().ok()) {
            c.perspective_presses = v.min(3);
        }
        if let Some(v) = g("mode") {
            c.mode = Mode::parse(&v);
        }
        if let Some(v) = g("attack_interval_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.attack_interval_ms = v.clamp(50, 10_000);
        }
        if let Some(v) = g("jump_crit_every").and_then(|v| v.parse::<u32>().ok()) {
            c.jump_crit_every = v;
        }
        if let Some(v) = g("strafe_flip_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.strafe_flip_ms = v.max(50);
        }
        if let Some(v) = g("look_rate").and_then(|v| v.parse::<f32>().ok()) {
            c.look_rate = v.clamp(0.0, 720.0);
        }
        if let Some(v) = g("low_hp").and_then(|v| v.parse::<f32>().ok()) {
            c.low_hp = v.clamp(0.05, 0.95);
        }
        if let Some(v) = g("patrol_jump_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.patrol_jump_ms = v.max(200);
        }
        if let Some(v) = g("look_gain").and_then(|v| v.parse::<f32>().ok()) {
            c.look_gain = v.clamp(0.05, 20.0);
        }
        if let Some(v) = g("invert_pitch").and_then(|v| parse_bool(&v)) {
            c.invert_pitch = v;
        }
        if let Some(v) = g("sprint").and_then(|v| parse_bool(&v)) {
            c.sprint = v;
        }
        if let Some(v) = g("assume_threat").and_then(|v| parse_bool(&v)) {
            c.assume_threat = v;
        }
        if let Some(v) = g("log_path") {
            c.log_path = PathBuf::from(v);
        }
        if let Some(v) = g("run_log") {
            c.run_log = PathBuf::from(v);
        }

        // 次操作开关
        if let Some(v) = g("ch_movement").and_then(|v| parse_bool(&v)) {
            c.channels.movement = v;
        }
        if let Some(v) = g("ch_look").and_then(|v| parse_bool(&v)) {
            c.channels.look = v;
        }
        if let Some(v) = g("ch_attack").and_then(|v| parse_bool(&v)) {
            c.channels.attack = v;
        }
        if let Some(v) = g("ch_jump").and_then(|v| parse_bool(&v)) {
            c.channels.jump = v;
        }
        if let Some(v) = g("tac_range").and_then(|v| parse_bool(&v)) {
            c.tactics.range = v;
        }
        if let Some(v) = g("tac_hazard").and_then(|v| parse_bool(&v)) {
            c.tactics.hazard = v;
        }
        if let Some(v) = g("tac_guard").and_then(|v| parse_bool(&v)) {
            c.tactics.guard = v;
        }
        if let Some(v) = g("tac_breach").and_then(|v| parse_bool(&v)) {
            c.tactics.breach = v;
        }
        if let Some(v) = g("ch_use").and_then(|v| parse_bool(&v)) {
            c.channels.use_item = v;
        }
        c
    }
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_kv(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().to_string();
            let mut val = v.trim().to_string();
            if val.len() >= 2
                && ((val.starts_with('"') && val.ends_with('"'))
                    || (val.starts_with('\'') && val.ends_with('\'')))
            {
                val = val[1..val.len() - 1].to_string();
            }
            // 去掉行尾注释（# 前面有空格才算）
            if let Some(pos) = val.find(" #") {
                val.truncate(pos);
                val = val.trim().to_string();
            }
            m.insert(key, val);
        }
    }
    m
}

pub const DEFAULT_TOML: &str = r#"# trit-agent 配置
# 所有键都可以删，删了就用默认值。

# 状态机内部节拍（Hz）。200 = 游戏 20 TPS 的 10 倍。上限 2000。
hz = 200

# 窗口标题关键字（不区分大小写），用来找到 Minecraft 窗口
window_title = "Minecraft"

# 启动后是否自动切第三视角（注入 F5）
third_person_on_start = true
# 按几次 F5：1 = 背视第三视角，2 = 前视第三视角
perspective_presses = 1

# 热键（虚拟键码）：F8 开关 AI，F9 急停并交还控制权，F6 开关攻击，F7 开关移动
kill_switch_vk = 119
panic_vk = 120
toggle_attack_vk = 117
toggle_move_vk = 118

# hold = 原地护主 / patrol = 向前巡逻 / follow = 跟随
mode = "hold"

# 「次操作」开关：false 表示这一项完全交给人类，AI 不碰
ch_movement = true
ch_look = true
ch_attack = true
ch_jump = true
ch_use = true

# 战术开关（全部移植自整合包里 mob mod 的 AI Goal，可单独关）
# 交战距离带：太远追 / 3~7 格站住 / 贴脸退半步
tac_range = true
# 爆炸物规避：苦力怕 10 格、TNT 3 格
tac_hazard = true
# 受伤且近身时按右键格挡（手上要是盾；拿食物会变成进食）
tac_guard = true
# 卡住 1.2 秒就低头砍墙
tac_breach = true

# 战斗节奏
attack_interval_ms = 620
jump_crit_every = 2

# 走位
strafe_flip_ms = 900
sprint = true

# 视角：转速（度/秒）。look_gain = 每度需要多少个鼠标像素，
# 默认灵敏度(0.5) 大约 1.0，灵敏度调高就调小 look_gain。
look_rate = 55
look_gain = 1.35
invert_pitch = false

# 生存
low_hp = 0.35
patrol_jump_ms = 2600

# 日志
log_path = ""
run_log = "trit-agent-run.log"

# 没有 trit_bridge 数据包的传感器数据时，是否允许「无脑打架」。
# false = 没感知就站桩（推荐，符合三值「未知不猜」原则）
assume_threat = false
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_from_shipped_toml() {
        let c = Config::from_str(DEFAULT_TOML);
        assert_eq!(c.hz, 200);
        assert_eq!(c.mode, Mode::Hold);
        assert!(c.third_person_on_start);
        assert_eq!(c.kill_switch_vk, 0x77);
        assert!(c.channels.attack);
    }

    #[test]
    fn override_and_comment_strip() {
        let c = Config::from_str("hz = 500  # 高频\nmode = patrol\nch_attack = false\n");
        assert_eq!(c.hz, 500);
        assert_eq!(c.mode, Mode::Patrol);
        assert!(!c.channels.attack);
        // 未提及的保持默认
        assert!(c.channels.look);
    }

    #[test]
    fn hz_is_clamped() {
        assert_eq!(Config::from_str("hz = 99999").hz, 2000);
        assert_eq!(Config::from_str("hz = 0").hz, 1);
    }
}
