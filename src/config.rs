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
            Mode::Hold => "原地待命",
            Mode::Patrol => "巡逻",
            Mode::Follow => "跟随",
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

/// 采集（砍树 / 挖矿）的判据参数。
///
/// 全部围绕一件事：**什么时候不许动手**。所以这里的阈值大多是"下限"，
/// 低于就不干，而不是"高于就干"。
#[derive(Clone, Copy, Debug)]
pub struct GatherCfg {
    /// 总开关。默认关 —— 采集会主动改变世界（砍掉你的树），
    /// 不该是默认行为。
    pub enabled: bool,
    /// 采什么：木头还是矿石。两者的门控和瞄准都不同。
    pub target: crate::gather::Target,
    /// 会不会按需打开「快通道」（每 100ms 一帧的感知）。
    ///
    /// 打开后瞄准闭环的等待时间从 1 秒掉到 100ms 量级，代价是动手期间
    /// 聊天框每秒刷 10 行。关掉它一切照常，只是瞄得慢、转视角会多试几次。
    /// 需要 `squad_control = chat` 才有打字通道。
    pub fast_bridge: bool,
    /// 血量低于这个比例就不采集。砍树时视线朝下、背对空地，挨打很难受。
    pub min_hp: f32,
    /// 饱食度低于等于这个值就不采集。饿着砍完跑不动，也不能回血。
    pub min_food: i32,
    /// 夜里是否照常采集。默认 false：夜间刷怪密度高好几倍。
    pub allow_night: bool,
    /// 一趟砍够多少棵就休息（0 = 不限制）
    pub quota: u32,
    /// 配额满了以后歇多久，然后自动开下一趟
    pub rest_ms: u64,
    /// 连续安全多少毫秒才允许开工。这个「攒窗口」是防抽搐的关键。
    pub enter_ms: u64,
    /// 被危险打断后多久不再开工
    pub cooldown_ms: u64,
    /// 采集过程中多久插播一次抬头扫视（0 = 不扫）
    pub scan_every_ms: u64,
    /// 每次扫视持续多久。这段时间不砍也不走，只转视角。
    pub scan_ms: u64,
    /// 走到这个距离（格）就开始砍。原版手臂能到 4.5，留点余量。
    pub reach: f32,
    /// 看得见但超过这个距离就不去（格）
    pub max_walk: f32,
    /// 单棵树砍多久还没断就放弃（毫秒）。判断错了别在那儿杵着。
    pub max_chop_ms: u64,
}

impl Default for GatherCfg {
    fn default() -> Self {
        GatherCfg {
            enabled: false,
            target: crate::gather::Target::Wood,
            fast_bridge: true,
            min_hp: 0.65,
            min_food: 7,
            allow_night: false,
            quota: 16,
            rest_ms: 30_000,
            enter_ms: 2_500,
            cooldown_ms: 8_000,
            scan_every_ms: 1_600,
            scan_ms: 350,
            reach: 3.2,
            max_walk: 20.0,
            max_chop_ms: 25_000,
        }
    }
}

/// 指令通道。决定「共同意识」用哪条路把意图传给从属实体。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlChannel {
    /// 完全不用，只算意图不下发
    Off,
    /// 只记录，不真发。想先看 AI 打算怎么指挥、又不想让它动的时候用
    Dry,
    /// 走游戏命令框真发。需要单人世界开作弊
    Chat,
}

impl ControlChannel {
    pub fn parse(s: &str) -> ControlChannel {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" | "command" | "cmd" | "游戏" => ControlChannel::Chat,
            "dry" | "log" | "只记录" => ControlChannel::Dry,
            _ => ControlChannel::Off,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ControlChannel::Off => "off",
            ControlChannel::Dry => "dry",
            ControlChannel::Chat => "chat",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// 状态机内部节拍。默认 200 Hz（游戏是 20 TPS，这里是它的 10 倍，
    /// 输入抖动被内部高频抹平）。
    pub hz: u32,
    pub window_title: String,
    /// 接管时切到第几视角。0 = 第一视角，1 = 第三视角背视，2 = 第三视角前视。
    pub perspective_presses: u8,
    /// 接管那一刻是否真的去切视角。关掉就完全不动你的视角。
    pub third_person_on_start: bool,
    /// 交还控制权（关 AI）时是否切回第一视角 —— 第三视角下人手操作很难受。
    pub restore_first_person: bool,
    /// 是否在游戏窗口标题前挂 `[AI 接管]` / `[已交还]` 标记。
    /// 这是「AI 现在到底听不听我的」的唯一即时反馈，别关。
    pub title_badge: bool,
    /// 游戏窗口不在前台时停止注入。防止 Alt+Tab 之后 AI 在别的窗口里乱敲。
    pub pause_when_unfocused: bool,

    pub kill_switch_vk: u16,
    pub panic_vk: u16,
    pub toggle_attack_vk: u16,
    pub toggle_move_vk: u16,
    pub toggle_perspective_vk: u16,

    pub mode: Mode,
    pub channels: Channels,
    pub tactics: Tactics,
    /// 战法（孙子兵法）。一次只生效一条，见 [`crate::strategy::select`]。
    pub strategies: crate::strategy::Strategies,

    /// 共同意识：一个大脑控多个身体
    pub squad: crate::squad::SquadCfg,
    pub squad_control: ControlChannel,

    /// 采集（砍树）。判据见 [`crate::gather`]。
    pub gather: GatherCfg,

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
            perspective_presses: 1,
            third_person_on_start: true,
            restore_first_person: true,
            title_badge: true,
            pause_when_unfocused: true,
            kill_switch_vk: 0x77,   // F8
            panic_vk: 0x78,         // F9
            toggle_attack_vk: 0x75, // F6
            toggle_move_vk: 0x76,   // F7
            toggle_perspective_vk: 0x79, // F10
            mode: Mode::Hold,
            channels: Channels::default(),
            tactics: Tactics::default(),
            strategies: crate::strategy::Strategies::default(),
            squad: crate::squad::SquadCfg::default(),
            squad_control: ControlChannel::Off,
            gather: GatherCfg::default(),
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
            c.perspective_presses = v % 3;
        }
        if let Some(v) = g("restore_first_person").and_then(|v| parse_bool(&v)) {
            c.restore_first_person = v;
        }
        if let Some(v) = g("title_badge").and_then(|v| parse_bool(&v)) {
            c.title_badge = v;
        }
        if let Some(v) = g("pause_when_unfocused").and_then(|v| parse_bool(&v)) {
            c.pause_when_unfocused = v;
        }
        if let Some(v) = g("toggle_perspective_vk").and_then(|v| v.parse::<u16>().ok()) {
            c.toggle_perspective_vk = v;
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

        // ---- 战法（孙子兵法）----
        if let Some(v) = g("strat_retreat").and_then(|v| parse_bool(&v)) {
            c.strategies.retreat = v;
        }
        if let Some(v) = g("strat_wait").and_then(|v| parse_bool(&v)) {
            c.strategies.wait = v;
        }
        if let Some(v) = g("strat_blitz").and_then(|v| parse_bool(&v)) {
            c.strategies.blitz = v;
        }
        if let Some(v) = g("strat_ambush").and_then(|v| parse_bool(&v)) {
            c.strategies.ambush = v;
        }
        if let Some(v) = g("strat_feint").and_then(|v| parse_bool(&v)) {
            c.strategies.feint = v;
        }
        if let Some(v) = g("strat_feint_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.strategies.feint_ms = v.clamp(150, 5_000);
        }

        // ---- 共同意识 ----
        if let Some(v) = g("squad").and_then(|v| parse_bool(&v)) {
            c.squad.enabled = v;
        }
        if let Some(v) = g("squad_control") {
            c.squad_control = ControlChannel::parse(&v);
        }
        if let Some(v) = g("squad_engage_dist").and_then(|v| v.parse::<f32>().ok()) {
            c.squad.engage_dist = v.clamp(2.0, 64.0);
        }
        if let Some(v) = g("squad_recall_hp").and_then(|v| v.parse::<f32>().ok()) {
            c.squad.recall_hp = v.clamp(0.05, 0.95);
        }
        if let Some(v) = g("squad_cmd_gap_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.squad.min_interval_ms = v.clamp(200, 30_000);
        }

        // ---- 采集 ----
        if let Some(v) = g("gather").and_then(|v| parse_bool(&v)) {
            c.gather.enabled = v;
        }
        if let Some(v) = g("gather_min_hp").and_then(|v| v.parse::<f32>().ok()) {
            c.gather.min_hp = v.clamp(0.1, 0.95);
        }
        if let Some(v) = g("gather_target").and_then(|v| crate::gather::Target::parse(&v)) {
            c.gather.target = v;
        }
        if let Some(v) = g("gather_fast_bridge").and_then(|v| parse_bool(&v)) {
            c.gather.fast_bridge = v;
        }
        if let Some(v) = g("gather_min_food").and_then(|v| v.parse::<i32>().ok()) {
            c.gather.min_food = v.clamp(0, 20);
        }
        if let Some(v) = g("gather_night").and_then(|v| parse_bool(&v)) {
            c.gather.allow_night = v;
        }
        if let Some(v) = g("gather_quota").and_then(|v| v.parse::<u32>().ok()) {
            c.gather.quota = v.min(1_000);
        }
        if let Some(v) = g("gather_rest_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.gather.rest_ms = v.clamp(1_000, 600_000);
        }
        if let Some(v) = g("gather_enter_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.gather.enter_ms = v.clamp(0, 60_000);
        }
        if let Some(v) = g("gather_cooldown_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.gather.cooldown_ms = v.clamp(0, 300_000);
        }
        if let Some(v) = g("gather_scan_every_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.gather.scan_every_ms = v.clamp(0, 60_000);
        }
        if let Some(v) = g("gather_scan_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.gather.scan_ms = v.clamp(0, 10_000);
        }
        if let Some(v) = g("gather_reach").and_then(|v| v.parse::<f32>().ok()) {
            // 下限 3.0 不是随便定的：数据包最近的原木探测档就是 3 格。
            // 允许设到 3 以下的话，`wood_dist(3) == 3.0 > reach` 永远成立，
            // 角色会一直往树里挤却永远不开始砍。
            c.gather.reach = v.clamp(3.0, 4.4);
        }
        if let Some(v) = g("gather_max_walk").and_then(|v| v.parse::<f32>().ok()) {
            c.gather.max_walk = v.clamp(2.0, 64.0);
        }
        if let Some(v) = g("gather_max_chop_ms").and_then(|v| v.parse::<u64>().ok()) {
            c.gather.max_chop_ms = v.clamp(1_000, 300_000);
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

# 接管时切到第几视角：0 = 第一视角，1 = 第三视角（背视），2 = 第三视角（前视）
# 切法是「先按满一圈归零、再按到目标」，所以不管你开局停在哪一档，结果都是确定的。
perspective_presses = 1
third_person_on_start = true
# 关掉 AI（F8）时自动切回第一视角 —— 第三视角下自己操作很难受。
restore_first_person = true

# 在游戏窗口标题前挂 [AI 接管] / [已交还] 标记，一眼看出 AI 开没开。
title_badge = true
# 游戏窗口不在前台时停止注入。防止 Alt+Tab 出去后 AI 在别的窗口乱敲键盘。
pause_when_unfocused = true

# 热键（虚拟键码）：
#   F8  = 119  开关 AI（按一下关，再按一下开）
#   F9  = 120  急停：关掉 + 强制松开所有键，不切回来
#   F6  = 117  单独关/开「攻击」这一项
#   F7  = 118  单独关/开「移动」这一项（前后+左右）
#   F10 = 121  单独切视角（AI 开/关时都可用）
kill_switch_vk = 119
panic_vk = 120
toggle_attack_vk = 117
toggle_move_vk = 118
toggle_perspective_vk = 121

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

# ---- 采集（砍树 / 挖矿）----
# 默认关：采集会主动改变世界（砍掉你的树、砍掉你的房子）。
# 打开后 AI 只在「明确安全」时动手，判据见 README「什么时候不该砍」。
gather = false
# 采什么：wood = 砍树，ore = 挖矿。换目标只改这一行。
# 挖矿比砍树多三条判据：脚下要有支撑、目标不能是岩浆、前方被墙挡就换目标。
gather_target = wood
# 会不会按需打开快通道（每 100ms 一帧）。开了瞄准准，代价是动手期间
# 聊天框每秒刷 10 行；不开也能用，只是瞄得慢。需要 squad_control = chat。
gather_fast_bridge = true
# 血低于这个比例就不砍（0.65 = 13 点血）
gather_min_hp = 0.65
# 饱食度低于等于这个数就不砍 —— 饿着砍完跑不动，也不能回血
gather_min_food = 7
# 夜里是否照常砍。默认 false：夜间刷怪密度高好几倍
gather_night = false
# 一趟砍够多少棵就休息（0 = 不限制）
gather_quota = 16
# 配额满了歇多久再开下一趟
gather_rest_ms = 30000
# 连续安全多少毫秒才允许开工。这个「攒窗口」是防止砍两下跑一下的关键
gather_enter_ms = 2500
# 被威胁打断后多久不再开工。没有这个，怪物一走远就回去砍、砍两下又跑
gather_cooldown_ms = 8000
# 采集过程中每多久抬头扫视一次，每次看多久（秒级，别设太小）
gather_scan_every_ms = 1600
gather_scan_ms = 350
# 走到这个距离就开始砍（原版手臂 4.5 格，留余量）
gather_reach = 3.2
# 看得见但超过这个距离就不去
gather_max_walk = 20.0
# 一棵树砍多久还没断就放弃（多半是判断错了：树在墙后/在坡上）
gather_max_chop_ms = 25000

# ---- 战法（孙子兵法）：一次只生效一条，优先级 走为上 > 以逸待劳 > 兵贵神速 > 围点打援 > 声东击西 ----
# 走为上：血 < 35%，或血 < 50% 且被两只以上围 → 后退脱离
strat_retreat = true
# 以逸待劳：血不满(75%以下)且敌人还在 10 格外 → 站着回血，等它走过来
strat_wait = true
# 兵贵神速：血 > 80% 且只有一只且距离 ≤ 12 格 → 全速压上，把距离带的"站住"压掉
strat_blitz = true
# 围点打援：两只以上且在 6 格外 → 不冲群，绕到侧面把最前那只放出来单杀
strat_ambush = true
# 声东击西：交战中贴身 6 格内 → 持续侧移，方向每 strat_feint_ms 翻转
strat_feint = true
strat_feint_ms = 700

# ---- 共同意识：一个大脑控多个身体 ----
# 打开后 AI 会同时指挥你的铁傀儡/狼（数据包 squad 用 tag 认人）
squad = false
# 指令通道：off = 只算不指挥 / dry = 只记录不真发 / chat = 走游戏命令框真发（需开作弊）
squad_control = "dry"
# 超过这个距离先让它们集合，不要各打各的（会被逐个击破）
squad_engage_dist = 12
# 低于这个血量且有威胁 → 召回护主
squad_recall_hp = 0.45
# 两条指令的最小间隔。指令是打字打进去的，太密会互相打断
squad_cmd_gap_ms = 1200

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
        // 视角与交还开关必须和默认值一致，否则关 AI 后会把人锁在第三视角里
        assert_eq!(c.perspective_presses, 1);
        assert!(c.restore_first_person);
        assert!(c.title_badge);
        assert!(c.pause_when_unfocused);
        assert_eq!(c.kill_switch_vk, 0x77); // F8
        assert_eq!(c.panic_vk, 0x78); // F9
        assert_eq!(c.toggle_perspective_vk, 0x79); // F10
    }

    #[test]
    fn perspective_presses_wraps_around() {
        // 3 和 0 是同一档（按满一圈回第一视角），别让 3 变成「按三次」的怪值
        assert_eq!(Config::from_str("perspective_presses = 3").perspective_presses, 0);
        assert_eq!(Config::from_str("perspective_presses = 2").perspective_presses, 2);
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
    fn every_gather_key_is_parsed() {
        // 键名写错/漏解析在这套自制 parser 里是**静默失败**（用默认值），
        // 所以逐个改成一个明显不同的值再读回来。少一个键这条就会挂。
        let c = Config::from_str(
            "gather = true
             gather_min_hp = 0.5
             gather_min_food = 3
             gather_night = true
             gather_quota = 4
             gather_rest_ms = 5000
             gather_enter_ms = 1000
             gather_cooldown_ms = 2000
             gather_scan_every_ms = 800
             gather_scan_ms = 100
             gather_reach = 3.5
             gather_max_walk = 9
             gather_max_chop_ms = 12000
",
        );
        assert!(c.gather.enabled);
        assert!((c.gather.min_hp - 0.5).abs() < 1e-6);
        assert_eq!(c.gather.min_food, 3);
        assert!(c.gather.allow_night);
        assert_eq!(c.gather.quota, 4);
        assert_eq!(c.gather.rest_ms, 5_000);
        assert_eq!(c.gather.enter_ms, 1_000);
        assert_eq!(c.gather.cooldown_ms, 2_000);
        assert_eq!(c.gather.scan_every_ms, 800);
        assert_eq!(c.gather.scan_ms, 100);
        assert!((c.gather.reach - 3.5).abs() < 1e-6);
        assert!((c.gather.max_walk - 9.0).abs() < 1e-6);
        assert_eq!(c.gather.max_chop_ms, 12_000);
    }

    #[test]
    fn gather_reach_cannot_go_below_the_near_probe_tier() {
        // 数据包最近档就是 3 格。reach 设到 3 以下，`d = 3.0 > reach` 永远成立，
        // 角色会一路走到树里也进不了"砍"的状态。夹到 3.0 起步。
        let c = Config::from_str("gather_reach = 1.2");
        assert!(
            c.gather.reach >= 3.0,
            "reach 低于最近探测档，就永远判定还没到，实际值 {}",
            c.gather.reach
        );
    }

    #[test]
    fn shipped_toml_matches_gather_defaults() {
        // 文档和实现脱节过一次（help 列了 init 但没实现），这里锁住。
        let shipped = Config::from_str(DEFAULT_TOML).gather;
        let builtin = GatherCfg::default();
        assert_eq!(shipped.enabled, builtin.enabled);
        assert_eq!(shipped.min_food, builtin.min_food);
        assert_eq!(shipped.allow_night, builtin.allow_night);
        assert_eq!(shipped.quota, builtin.quota);
        assert_eq!(shipped.enter_ms, builtin.enter_ms);
        assert_eq!(shipped.cooldown_ms, builtin.cooldown_ms);
    }

    #[test]
    fn hz_is_clamped() {
        assert_eq!(Config::from_str("hz = 99999").hz, 2000);
        assert_eq!(Config::from_str("hz = 0").hz, 1);
    }
}
