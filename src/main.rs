//! trit-agent 命令行。
//!
//! ```text
//! trit-agent selftest         # 逻辑自检 + 高频实测（不开游戏）
//! trit-agent sim              # 场景回放，看决策怎么变（不开游戏）
//! trit-agent probe            # 找窗口 / 查日志路径
//! trit-agent run              # 真接管
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};
use trit_agent::act::dry::DryInput;
use trit_agent::agent::Agent;
use trit_agent::config::{Config, Mode, DEFAULT_TOML};
use trit_agent::fsm::{Channel, Machine, Phase, Vote, WorldView};
use trit_agent::perceive::logtail::LogTail;
use trit_agent::perceive::{Block, LogEvent, SensorPatch};
use trit_agent::trit::{from_balanced, to_balanced, trits_str, Tri};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let flags = Flags::parse(&args[1.min(args.len())..]);

    match cmd {
        "selftest" => selftest(&flags),
        "sim" if flags.has("gather") => sim_gather(&flags),
        "sim" if flags.has("mine") => sim_mine(&flags),
        "sim" => sim(&flags),
        "probe" => probe(&flags),
        "run" => run(&flags),
        "init" => init(&flags),
        "help" | "-h" | "--help" => help(),
        _ => {
            eprintln!("未知命令: {cmd}\n");
            help();
        }
    }
}

/// 写出默认配置。之前 help 里列了这个命令但 match 里没有分支，
/// 敲 `trit-agent init` 会得到「未知命令」—— 属于文档比实现超前。
fn init(flags: &Flags) {
    let path = PathBuf::from(flags.get("config").unwrap_or("trit-agent.toml"));
    if path.exists() {
        eprintln!("{} 已存在，不覆盖。要重置就先手动删掉它。", path.display());
        return;
    }
    match std::fs::write(&path, DEFAULT_TOML) {
        Ok(_) => println!("已写出 {}", path.display()),
        Err(e) => eprintln!("写失败（{}）: {e}", path.display()),
    }
}

fn help() {
    println!(
        r#"trit-agent — 三值决策状态机 · Minecraft 角色托管

用法:
  trit-agent selftest [--hz 200] [--secs 2]   逻辑自检 + 高频节拍实测
  trit-agent sim [--verbose]                  离线场景回放（战斗 / 战法）
  trit-agent sim --gather                     离线回放「采集门控」：什么时候不该砍
  trit-agent probe [--config path] [--profile <整合包目录>]
  trit-agent run   [--config path] [--seconds N] [--hz N] [--mode hold|patrol|follow]
                   [--profile <整合包目录>] [--window 关键字] [--log path]
  trit-agent init                             生成默认配置文件

热键（运行中，全局有效）:
  F8   开关 AI —— ★ 一键交还控制权，再按一下重新接管
       关掉时会：立刻松开所有键 → 切回第一视角 → 标题变成 [AI 待机]
  F9   急停 —— 关掉 AI，但不切视角（想保持第三视角看场面时用）
  F10  切视角（第一 → 第三背视 → 第三前视 → 循环）
       和游戏内 F5 同效，但走 agent 自己的账面，AI 关着也能按
  F6   单独关/开「攻击」这一项
  F7   单独关/开「移动」这一项

第三视角:
  接管时自动切到第三视角（默认背视）。切法是「按满一圈归零再走到目标」，
  所以不管你开局停在哪一档，落点都是确定的。交还时默认切回第一视角。

窗口失焦时 AI 自动停手（避免 Alt+Tab 出去后往别的窗口里乱敲）。

游戏内聊天指令（AI 开着时直接打字）:
  !ai on / !ai off      !status
  !mode hold|patrol|follow
  !attack off   !move off   !look off   !jump off   !use off
  !tac off             四条移植战术（关掉 = 回到没那一档的原版行为）
  !strat               战法；!strat off 全关，!strat ambush 单独翻
  !squad               共同意识；!squad on|off，!squad recall 手动下令
  !gather on|off       采集（砍树）；!gather reset 清配额和冷却，!gather 8 设配额
  !gather night        夜里也砍（默认不砍）
  !threat       !calm       !panic

战法（孙子兵法，一次只生效一条，优先级从高到低）:
  走为上    血 <35%，或 <50% 且被 2 只以上围 → 后退脱离
  以逸待劳  血 <75% 且敌人还在 10 格外 → 站着回血，别残血接战
  兵贵神速  血 >80% 且单敌且 6~12 格 → 全速压上（进了交战带就交回距离带）
  围点打援  2 只以上且 6 格外 → 不冲群，绕侧把最前那只放出来单杀
  声东击西  贴身 6 格内 → 持续侧移，每 700ms 换向

共同意识（一个大脑控多个身体）:
  打开 squad 后，同一个决策核同时指挥你和你的傀儡/狼。
  执行端是数据包 squad：/function squad:recruit 收编，dismiss 解散。
  傀儡和狼没有输入通道，只能改 AngryAt —— 所以是「意图级」不是逐帧。
  squad_control: off 只算不指挥 / dry 只记录 / chat 走命令框真发（需开作弊）。
  想接对讲机、语音、第二屏：见 docs/CONTROLS.md。
"#
    );
}

struct Flags {
    map: std::collections::BTreeMap<String, String>,
}

impl Flags {
    fn parse(args: &[String]) -> Flags {
        let mut map = std::collections::BTreeMap::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if let Some(name) = a.strip_prefix("--") {
                if let Some((k, v)) = name.split_once('=') {
                    map.insert(k.to_string(), v.to_string());
                } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    map.insert(name.to_string(), args[i + 1].clone());
                    i += 1;
                } else {
                    map.insert(name.to_string(), "true".to_string());
                }
            }
            i += 1;
        }
        Flags { map }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(|s| s.as_str())
    }
    fn u64_of(&self, k: &str, default: u64) -> u64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn has(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }
}

fn load_cfg(flags: &Flags) -> Config {
    let path = PathBuf::from(flags.get("config").unwrap_or("trit-agent.toml"));
    let mut cfg = match Config::load_or_create(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("读配置失败（用默认值）: {e}");
            Config::from_str(DEFAULT_TOML)
        }
    };
    if let Some(hz) = flags.get("hz").and_then(|v| v.parse::<u32>().ok()) {
        cfg.hz = hz.clamp(1, 2000);
    }
    if let Some(p) = flags.get("log") {
        cfg.log_path = PathBuf::from(p);
    }
    if let Some(w) = flags.get("window") {
        // 之前报错文案里写了 `--window`，但这里根本没解析它 —— 提示了一个
        // 不存在的参数，比不提示更糟。
        cfg.window_title = w.to_string();
    }
    if let Some(profile) = flags.get("profile") {
        cfg.log_path = PathBuf::from(profile).join("logs").join("latest.log");
    }
    if let Some(m) = flags.get("mode") {
        cfg.mode = Config::from_str(&format!("mode = {m}")).mode;
    }
    cfg
}

// ---------------------------------------------------------------- selftest

fn selftest(flags: &Flags) {
    let mut pass = 0;
    let mut fail = 0;
    let mut check = |name: &str, ok: bool| {
        if ok {
            pass += 1;
            println!("  √ {name}");
        } else {
            fail += 1;
            println!("  × {name}");
        }
    };

    println!("[1/4] K3 逻辑");
    check("Neg.and(Zero)=Neg", Tri::Neg.and(Tri::Zero) == Tri::Neg);
    check("Pos.or(Zero)=Pos", Tri::Pos.or(Tri::Zero) == Tri::Pos);
    check("Zero 不会被猜成 Pos", Tri::Zero.and(Tri::Zero) != Tri::Pos);
    check("双重否定还原", Tri::Neg.not().not() == Tri::Neg);

    println!("[2/4] 平衡三进制");
    let mut rt_ok = true;
    for v in -500i64..=500 {
        if from_balanced(&to_balanced(v)) != v {
            rt_ok = false;
            break;
        }
    }
    check("[-500,500] 编解码往返", rt_ok);
    check("5 -> \"--+\"", trits_str(&to_balanced(5)) == "--+");

    println!("[3/4] 裁决器");
    let all = [true; 7];
    {
        let mut m = Machine::new();
        m.set_armed(true);
        let w = WorldView {
            hp: Some(1.0),
            threat: Tri::Neg,
            ..Default::default()
        };
        let d1 = m.step(&w, &[Vote::new(Channel::Forward, Tri::Pos, 2, "t")], &all);
        let d2 = m.step(&w, &[Vote::new(Channel::Forward, Tri::Neg, 1, "t")], &all);
        check(
            "迟滞：反向弱信号不翻向",
            d1.dirs[Channel::Forward.index()] == Tri::Pos && d2.dirs[Channel::Forward.index()] == Tri::Pos,
        );
        let d3 = m.step(
            &w,
            &[
                Vote::new(Channel::Attack, Tri::Pos, 1, "t"),
                Vote::abstain(Channel::Attack, 3, "s"),
            ],
            &all,
        );
        check("未知压过已知 → 弃权", d3.dirs[Channel::Attack.index()] == Tri::Zero);
    }

    println!("[4/4] 高频节拍");
    let hz = flags.u64_of("hz", 200) as u32;
    let secs = flags.u64_of("secs", 2).max(1) as f64;
    let (ticks, elapsed) = high_freq_bench(hz, Duration::from_secs_f64(secs));
    let achieved = ticks as f64 / elapsed.as_secs_f64();
    println!(
        "  目标 {hz} Hz · 实际 {achieved:.1} Hz · {ticks} 拍 / {:.3}s",
        elapsed.as_secs_f64()
    );
    check(
        format!("达成率 >= 80%（{:.0}%）", 100.0 * achieved / hz as f64).as_str(),
        achieved >= hz as f64 * 0.8,
    );

    println!("\n{} 通过 / {} 失败", pass, fail);
    if fail > 0 {
        std::process::exit(1);
    }
}

/// 只跑决策 + 定时，不碰硬件。用来证明「高频」不是嘴上说说。
fn high_freq_bench(hz: u32, dur: Duration) -> (u64, Duration) {
    #[cfg(windows)]
    trit_agent::act::win32::begin_high_res_timer();

    let cfg = Config::from_str(&format!("hz = {hz}\nassume_threat = true\n"));
    let mut a = Agent::new(cfg, Box::new(DryInput::new()));
    a.arm(true);
    a.ingest(
        LogEvent::Sensor(SensorPatch {
            hp: Some(20.0),
            food: Some(17),
            pos: Some((0.0, 64.0, 0.0)),
            hostiles: Some(2),
            dist_tier: Some(3),
            hazard_tier: Some(0),
            wood_tier: Some(0),
            wood_dir: Some(0),
            tod: Some(8_000),
            pitch: Some(0.0),
            ..Default::default()
        }),
        0,
    );

    let period = Duration::from_secs_f64(1.0 / hz as f64);
    let start = Instant::now();
    let mut ticks = 0u64;
    loop {
        let now = Instant::now();
        let now_ms = now.duration_since(start).as_millis() as u64;
        a.step(now_ms, (1000 / hz.max(1) as u64).max(1));
        ticks += 1;
        if now.duration_since(start) >= dur {
            break;
        }
        let target = period.mul_f64(ticks as f64);
        let elapsed = now.duration_since(start);
        if elapsed < target {
            let remain = target - elapsed;
            if remain > Duration::from_millis(2) {
                std::thread::sleep(remain - Duration::from_millis(1));
            }
            while Instant::now().duration_since(start) < target {
                std::hint::spin_loop();
            }
        }
    }
    (ticks, start.elapsed())
}

// ---------------------------------------------------------------- sim

fn sim(flags: &Flags) {
    let verbose = flags.has("verbose");
    let mut cfg = Config::default();
    cfg.assume_threat = false;
    if let Some(m) = flags.get("mode") {
        cfg.mode = Mode::parse(m);
    }
    println!("mode = {}
", cfg.mode.as_str());
    let mut a = Agent::new(cfg, Box::new(DryInput::new()));
    a.arm(true);

    // 场景表：(起, 止, 敌人数, 距离档, 爆炸源档, 说明)
    // 距离档 32/16/10/6/3 对应「最近敌人 ≤N 格」，和 trit_bridge 的输出一致。
    // 最后一位：坐标是否冻结（模拟顶着墙走不动）
    let scenes: [(u64, u64, i32, i32, i32, bool, &str); 8] = [
        (0, 2_000, 0, 0, 0, false, "无敌人"),
        (2_000, 4_000, 1, 32, 0, false, "敌人 ≤32 格 → 追"),
        (4_000, 5_000, 1, 10, 0, false, "敌人 ≤10 格 → 继续压上"),
        (5_000, 7_000, 1, 6, 0, false, "敌人 ≤6 格 → 站住（原版没有这一档）"),
        (7_000, 8_000, 1, 3, 0, false, "敌人 ≤3 格 → 退半步"),
        (8_000, 9_000, 2, 10, 3, false, "敌人在 10 格 + 苦力怕贴脸 → 规避压过追击"),
        (9_000, 10_400, 0, 0, 0, false, "敌人消失 → 停"),
        (10_400, 13_000, 1, 32, 0, true, "被墙挡住：想追但走不动 → 低头砍墙"),
    ];

    println!(
        "{}",
        row(&[
            h("时间", 6),
            h("阶段", 6),
            h("前", 3),
            h("横", 3),
            h("转", 3),
            h("仰", 3),
            h("攻", 3),
            h("跳", 3),
            h("用", 3),
            h("战法", 10),
            h("说明", 0),
        ])
    );
    let mut last: Option<[Tri; 7]> = None;
    let mut last_phase = Phase::Off;
    let mut last_strat = "";
    let mut mz = 0.0f32;
    let mut mx = 0.0f32; // 模拟坐标：真的会随按键移动，否则卡住检测会误报
    let mut next_sense = 0u64;

    for t in (0..=13_000).step_by(100) {
        let scene = scenes
            .iter()
            .find(|s| t >= s.0 && t < s.1)
            .copied()
            .unwrap_or(scenes[scenes.len() - 1]);

        // 传感器每秒上报一次（数据包是 20 tick = 1 秒）
        if t >= next_sense {
            a.ingest(
                LogEvent::Sensor(SensorPatch {
                    hp: Some(20.0),
                    food: Some(17),
                    pos: Some((mx, 64.0, mz)),
                    hostiles: Some(scene.2),
                    dist_tier: Some(scene.3),
                    hazard_tier: Some(scene.4),
                    // 战法演示的场景里没有木头；采集用 `sim gather` 单独演
                    wood_tier: Some(0),
                    wood_dir: Some(0),
                    tod: Some(8_000),
                    pitch: Some(0.0),
                    ..Default::default()
                }),
                t,
            );
            next_sense = t + 1_000;
        }

        let d = a.step(t, 5);
        let strat = a.active_strategy().map(|s| s.id()).unwrap_or("-");
        let changed = match last {
            None => true,
            Some(p) => p != d.dirs || d.phase != last_phase || strat != last_strat,
        };
        if changed || verbose {
            let dirs = &d.dirs;
            println!(
                "{}",
                row(&[
                    r(&t.to_string(), 6),
                    l(d.phase.zh(), 6),
                    h(&dirs[Channel::Forward.index()].ch().to_string(), 3),
                    h(&dirs[Channel::Strafe.index()].ch().to_string(), 3),
                    h(&dirs[Channel::Yaw.index()].ch().to_string(), 3),
                    h(&dirs[Channel::Pitch.index()].ch().to_string(), 3),
                    h(&dirs[Channel::Attack.index()].ch().to_string(), 3),
                    h(&dirs[Channel::Jump.index()].ch().to_string(), 3),
                    h(&dirs[Channel::Use.index()].ch().to_string(), 3),
                    l(strat, 10),
                    l(
                        &format!(
                            "{}{}",
                            scene.6,
                            if a.world(t, t).stuck { "  [卡住]" } else { "" }
                        ),
                        0
                    ),
                ]),
            );
            last = Some(*dirs);
            last_phase = d.phase;
            last_strat = strat;
        }

        // 模拟位移：前进约 4.3 格/秒，横移约 3 格/秒。
        // 两个方向都要模拟，否则「只按横移不产生前后位移」会被卡住检测误判成
        // 撞牆，然后低头砍地 —— 那是真的会发生的，但演示例子里不该出现。
        if !scene.5 {
            match d.dirs[Channel::Forward.index()] {
                Tri::Pos => mx += 0.43,
                Tri::Neg => mx -= 0.43,
                Tri::Zero => {}
            }
            match d.dirs[Channel::Strafe.index()] {
                Tri::Pos => mz += 0.30,
                Tri::Neg => mz -= 0.30,
                Tri::Zero => {}
            }
        }
    }

    println!();
    println!("「前」这一列有三段：+) 追上去   o) 站住不动   -) 后退");
    println!("中间那一段是这次从 mob mod 移植过来的 —— 原版 Goal 只有「靠近」和「开火」，");
    println!("没有「位置刚好」这一档，所以 NPC 会一路挤到你脸上。");
    println!("x 估算位移 {} 格（13 秒）", (mx * 10.0).round() / 10.0);
}

// ---------------------------------------------------------------- sim --gather

// ---------------------------------------------------------------- 演示表格

/// 单元格对齐方式。中文占两列，数字要右对齐才对得齐末尾。
#[derive(Clone, Copy)]
enum Align {
    Left,
    Right,
    Center,
}

/// 按**显示列宽**拼一行表格。
///
/// `format!("{:<7}", "砍伐")` 补 5 个空格，显示出来却有 9 列 —— 中文占两列，
/// 而 `{:<}` 数的是字符数。演示表格要给人看，对不齐就等于没做，
/// 所以这里统一走 [`trit_agent::text`] 的列宽函数。
///
/// 宽度传 0 表示「这是最后一列，不补空格」。
fn row(cells: &[(String, usize, Align)]) -> String {
    let mut out = String::new();
    for (i, (s, w, a)) in cells.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if *w == 0 {
            out.push_str(s);
            continue;
        }
        out.push_str(&match a {
            Align::Left => trit_agent::text::lpad(s, *w),
            Align::Right => trit_agent::text::rpad(s, *w),
            Align::Center => trit_agent::text::cpad(s, *w),
        });
    }
    out
}

/// 表头的一格：居中。
fn h(s: &str, w: usize) -> (String, usize, Align) {
    (s.to_string(), w, Align::Center)
}

/// 文字的格子：左对齐。
fn l(s: &str, w: usize) -> (String, usize, Align) {
    (s.to_string(), w, Align::Left)
}

/// 数字的格子：右对齐。
fn r(s: &str, w: usize) -> (String, usize, Align) {
    (s.to_string(), w, Align::Right)
}

/// 「攻击」那一列的三种状态：按住砍 / 单击 / 不按。
fn atk_cell(d: &trit_agent::fsm::Decision) -> &'static str {
    if d.attack_hold {
        "按住"
    } else if d.dirs[Channel::Attack.index()] == Tri::Pos {
        "点"
    } else {
        "-"
    }
}

/// 采集演示：把「什么时候不该砍」逐秒演一遍。
///
/// 这不是砍树的动画，是**门控的时间线**。每一行都能对上一条判据：
/// 走过去 → 开砍 → 冒怪收手 → 冷却期内怪走了也不回去 → 掉血停手 → 天黑停手。
fn sim_gather(flags: &Flags) {
    let verbose = flags.has("verbose");
    let cfg = Config::from_str("hz = 200
gather = true
mode = hold
");
    let mut a = Agent::new(cfg, Box::new(DryInput::new()));
    a.arm(true);

    // (起, 止, 木头档, 敌人数, 距离档, 血量, 游戏内时间, 说明)
    // 冷却默认 8 秒，所以"怪走了"那段必须给足 8 秒，否则演示里根本看不到
    // 「回来接着砍」这一拍 —— 时间线要跟着真实参数走，不能凭感觉编。
    let scenes: [(u64, u64, i32, i32, i32, f32, i32, &str); 7] = [
        (0, 3_000, 6, 0, 0, 20.0, 8_000, "安全，树在 6 格 → 走过去"),
        (3_000, 6_000, 3, 0, 0, 20.0, 8_000, "到 3 格 → 按住左键砍"),
        (6_000, 8_000, 3, 1, 6, 20.0, 8_000, "冒出一只僵尸 → 立刻收手"),
        (8_000, 14_000, 3, 0, 0, 20.0, 8_000, "怪走了，但冷却 8 秒没完 → 不回去砍"),
        // 冷却结束后还要重新攒 2.5 秒安全窗口，所以这里必须留满
        (14_000, 19_000, 3, 0, 0, 20.0, 8_000, "冷却过了 → 重新攒窗口，然后接着砍"),
        (19_000, 21_000, 3, 0, 0, 9.0, 8_000, "掉到 45% 血 → 停手"),
        (21_000, 23_000, 3, 0, 0, 20.0, 15_000, "血回满但天黑了 → 夜里不砍"),
    ];

    println!(
        "{}",
        row(&[
            h("时间", 6),
            h("阶段", 6),
            h("采集", 8),
            h("原因", 10),
            h("前", 4),
            h("仰", 5),
            h("攻击", 5),
            h("说明", 0),
        ])
    );
    let mut last: Option<(Phase, trit_agent::gather::Mode, trit_agent::gather::Why, Tri, Tri, bool)> =
        None;
    // 模拟的视线俯仰角。真实环境里这是数据包报上来的绝对值；
    // 这里按 look_rate 推算，好让演示能看出「闭环对准」在动。
    let mut cur_pitch = 0.0f32;

    for t in (0..=23_000).step_by(100) {
        let sc = scenes
            .iter()
            .find(|s| t >= s.0 && t < s.1)
            .copied()
            .unwrap_or(scenes[scenes.len() - 1]);

        // ---- 快通道：每 100ms 一帧，只报俯仰角 ----
        // 关键是**延迟一帧**：`cur_pitch` 在这一拍 step 之后才更新，所以这里
        // ingest 进去的是上一拍结算后的角度，等于模拟了「100ms 前看到的世界」。
        // 不模拟这个延迟的话，闭环永远是瞬间对齐，演示什么也看不出。
        a.ingest(
            LogEvent::Sensor(SensorPatch {
                pitch: Some(cur_pitch),
                fast: true,
                ..Default::default()
            }),
            t,
        );

        // ---- 慢通道：每 1 秒一帧，报资源 / 血量 / 敌人 ----
        if t % 1_000 == 0 {
            a.ingest(
                LogEvent::Sensor(SensorPatch {
                    hp: Some(sc.5),
                    food: Some(18),
                    pos: Some((0.0, 64.0, 0.0)),
                    hostiles: Some(sc.3),
                    dist_tier: Some(sc.4),
                    hazard_tier: Some(0),
                    wood_tier: Some(sc.2),
                    wood_dir: Some(0),
                    tod: Some(sc.6),
                    ..Default::default()
                }),
                t,
            );
        }

        let d = a.step(t, 5);
        // 每拍 100ms，look_rate 55°/s ≈ 5.5°/拍
        if d.dirs[Channel::Pitch.index()] != Tri::Zero {
            cur_pitch += d.dirs[Channel::Pitch.index()].to_f32() * 5.5;
            cur_pitch = cur_pitch.clamp(-90.0, 90.0);
        }
        // phase 也要进 key：否则「交战 → 停战」那一拍不会打出来，
        // 看起来像 AI 一直卡在交战里。
        let key = (
            d.phase,
            a.gather.mode,
            a.gather.why,
            d.dirs[Channel::Forward.index()],
            d.dirs[Channel::Pitch.index()],
            d.attack_hold,
        );
        if last != Some(key) || verbose {
            println!(
                "{} {} [俯仰={:>4.0}]",
                row(&[
                    r(&t.to_string(), 6),
                    l(d.phase.zh(), 6),
                    l(a.gather.mode.name(), 8),
                    l(a.gather.why.as_str(), 10),
                    h(&d.dirs[Channel::Forward.index()].ch().to_string(), 4),
                    h(&d.dirs[Channel::Pitch.index()].ch().to_string(), 5),
                    l(atk_cell(&d), 5),
                ]),
                sc.7,
                cur_pitch
            );
            last = Some(key);
        }
    }

    println!();
    println!(
        "共砍 {} 棵，被打断 {} 次（最后一次：{}）",
        a.gather.chopped,
        a.gather.interrupted,
        a.gather.interrupted_by.as_str()
    );
    println!("说明：atk 列「按住」= 正在砍方块，「点」= 单次挥击，「-」= 不按。");
}

// ---------------------------------------------------------------- sim --mine

/// 挖矿演示：把「挖矿比砍树多出来的那三条」逐秒演一遍。
///
/// 砍树站着不动就行，挖矿是在改自己脚下的地形，所以每一条拒绝都有具体的
/// 死法在后面：悬空挖 = 掉进自己挖的坑，脚踩岩浆 = 掉进去就死，对着墙走 =
/// 原地推墙十秒。这个演示就是让这三条在时间线上各出现一次。
fn sim_mine(flags: &Flags) {
    let verbose = flags.has("verbose");
    // 冷却默认是 8 秒（真实环境里就该这么长 —— 危险刚过就回去挖是最蠢的）。
    // 演示里压到 2 秒，否则一次打断就把后面三种死法全盖住了，什么也看不出。
    let cfg = Config::from_str("hz = 200
gather = true
gather_target = ore
gather_cooldown_ms = 2000
mode = hold
");
    let mut a = Agent::new(cfg, Box::new(DryInput::new()));
    a.arm(true);

    // (起, 止, 矿档, 脚下方块, 准星方块, 墙体, 说明)
    //
    // 墙体掩码 1 = 正前方 1 格是固体。注意「准星对着矿石」时那条掩码也一定是 1
    // （矿石本来就是固体），所以 walk 阶段到底算不算被挡，靠的是 blk_front：
    // 对着能挖的东西就不算挡路。
    let scenes: [(u64, u64, i32, Block, Block, u8, &str); 6] = [
        (0, 3_000, 4, Block::Solid, Block::Empty, 0, "矿在 4 格 → 走过去"),
        (3_000, 6_000, 2, Block::Solid, Block::Ore, 1, "到 2 格，准星对上矿石 → 开挖"),
        (6_000, 8_000, 2, Block::Empty, Block::Ore, 1, "脚下被挖空 → 立刻停手（掉坑）"),
        (8_000, 11_000, 8, Block::Solid, Block::Solid, 1, "前方是实心墙，没有矿 → 不硬走"),
        (11_000, 13_000, 2, Block::Solid, Block::Lava, 1, "准星对着岩浆 → 绝对不挖"),
        (13_000, 20_000, 2, Block::Solid, Block::Ore, 1, "恢复正常 → 继续挖"),
        ];

    println!(
        "{}",
        row(&[
            h("时间", 6),
            h("阶段", 6),
            h("采集", 8),
            h("原因", 10),
            h("前", 4),
            h("仰", 5),
            h("攻击", 5),
            h("说明", 0),
        ])
    );
    let mut last: Option<(Phase, trit_agent::gather::Mode, trit_agent::gather::Why, Tri, Tri, bool)> =
        None;
    let mut cur_pitch = 0.0f32;

    for t in (0..=20_000).step_by(100) {
        let sc = scenes
            .iter()
            .find(|s| t >= s.0 && t < s.1)
            .copied()
            .unwrap_or(scenes[scenes.len() - 1]);

        // 快通道：100ms 一帧，含 1 帧延迟
        a.ingest(
            LogEvent::Sensor(SensorPatch {
                pitch: Some(cur_pitch),
                wall: Some(sc.5),
                blk_front: Some(sc.4),
                blk_below: Some(sc.3),
                fast: true,
                ..Default::default()
            }),
            t,
        );
        // 慢通道：1 秒一帧。矿在最后一段消失 —— 用它触发「挖掉一个」的计数。
        if t % 1_000 == 0 {
            // 17 秒时矿挖穿了。要等慢通道（1 秒一帧）报上来才知道，
            // 所以「挖掉一个」的计数一定滞后 ≤1 秒 —— 这是感知频率的下限，
            // 不是逻辑 bug。
            let ore_tier = if t >= 17_000 { 0 } else { sc.2 };
            a.ingest(
                LogEvent::Sensor(SensorPatch {
                    hp: Some(20.0),
                    food: Some(18),
                    pos: Some((0.0, 12.0, 0.0)),
                    hostiles: Some(0),
                    dist_tier: Some(0),
                    hazard_tier: Some(0),
                    ore_tier: Some(ore_tier),
                    ore_dir: Some(0),
                    tod: Some(6_000),
                    ..Default::default()
                }),
                t,
            );
        }

        let d = a.step(t, 5);
        if d.dirs[Channel::Pitch.index()] != Tri::Zero {
            cur_pitch += d.dirs[Channel::Pitch.index()].to_f32() * 5.5;
            cur_pitch = cur_pitch.clamp(-90.0, 90.0);
        }
        let key = (
            d.phase,
            a.gather.mode,
            a.gather.why,
            d.dirs[Channel::Forward.index()],
            d.dirs[Channel::Pitch.index()],
            d.attack_hold,
        );
        if last != Some(key) || verbose {
            println!(
                "{} {} [俯仰={:>4.0}]",
                row(&[
                    r(&t.to_string(), 6),
                    l(d.phase.zh(), 6),
                    l(a.gather.mode.name(), 8),
                    l(a.gather.why.as_str(), 10),
                    h(&d.dirs[Channel::Forward.index()].ch().to_string(), 4),
                    h(&d.dirs[Channel::Pitch.index()].ch().to_string(), 5),
                    l(atk_cell(&d), 5),
                ]),
                sc.6,
                cur_pitch
            );
            last = Some(key);
        }
    }

    println!();
    println!(
        "共挖 {} 个，被打断 {} 次（最后一次：{}）",
        a.gather.chopped,
        a.gather.interrupted,
        a.gather.interrupted_by.as_str()
    );
    println!(
        "感知延迟实测 {:.0}ms —— 这个数进「推一下 → 停手等 → 再推」的等待时间，\n\
         所以它直接决定瞄准要试几次。快通道 100ms 档大约是慢通道 1 秒档的三分之一。",
        a.sense_gap_ms.unwrap_or(-1.0)
    );
}

// ---------------------------------------------------------------- probe

fn probe(flags: &Flags) {
    let cfg = load_cfg(flags);
    println!("配置 hz={} mode={} assume_threat={}", cfg.hz, cfg.mode.as_str(), cfg.assume_threat);

    #[cfg(windows)]
    {
        trit_agent::act::win32::begin_high_res_timer();
        match trit_agent::act::win32::find_window(&cfg.window_title) {
            Some((h, t)) => println!("窗口: 0x{h:x} 标题=\"{t}\""),
            None => {
                println!(
                    "窗口: 未找到游戏窗口（类名 GLFW* 或标题含 \"{}\"）",
                    cfg.window_title
                );
                println!("候选窗口（前 20 个）：");
                for w in trit_agent::act::win32::list_windows().into_iter().take(20) {
                    println!("  - {w}");
                }
            }
        }
    }

    let p = if cfg.log_path.as_os_str().is_empty() {
        PathBuf::from("logs/latest.log")
    } else {
        cfg.log_path.clone()
    };
    println!(
        "日志: {} ({})",
        p.display(),
        if p.exists() { "存在" } else { "不存在 —— 聊天感知不可用" }
    );
}

// ---------------------------------------------------------------- run

fn run(flags: &Flags) {
    let mut cfg = load_cfg(flags);
    cfg.assume_threat = flags.has("assume-threat") || cfg.assume_threat;
    let seconds = flags.u64_of("seconds", 0);

    #[cfg(not(windows))]
    {
        let _ = (cfg, seconds);
        eprintln!("run 只能在 Windows 上跑：需要 SendInput 注入。");
        std::process::exit(2);
    }

    #[cfg(windows)]
    {
        use trit_agent::act::win32::{begin_high_res_timer, end_high_res_timer, Hotkeys, Win32Input};

        begin_high_res_timer();
        trit_agent::act::win32::install_panic_handler();

        // 等窗口，别一找不到就退出。
        //
        // 双击 bat 的那一下游戏通常还在加载（整合包要几十秒），旧代码此时打印
        // 一行就 exit(2)，黑窗口一闪而过 —— 玩家看到的就是「F8 按了没反应」，
        // 因为进程早没了。现在改成原地等，每 10 秒说一次在等什么。
        let input = {
            let deadline = (seconds > 0).then(|| Instant::now() + Duration::from_secs(seconds));
            let mut note = Instant::now();
            let mut tries: u32 = 0;
            loop {
                if let Some(i) = Win32Input::attach(&cfg.window_title) {
                    break i;
                }
                tries += 1;
                if note.elapsed() >= Duration::from_secs(10) || tries == 1 {
                    note = Instant::now();
                    println!(
                        "还在等游戏窗口（标题含 \"{}\"）……已等 {} 秒。\
                         先把游戏开起来并进入世界，这个窗口不要关。",
                        cfg.window_title,
                        (tries as f32 * 0.5) as u32
                    );
                }
                if let Some(d) = deadline {
                    if Instant::now() >= d {
                        eprintln!("等了 {} 秒仍找不到窗口，退出。", seconds);
                        end_high_res_timer();
                        std::process::exit(2);
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        };
        println!("已挂上窗口: {}", input.title());

        let mut agent = Agent::new(cfg, Box::new(input));
        agent.arm(true);

        let log_path = if agent.cfg.log_path.as_os_str().is_empty() {
            PathBuf::from("logs/latest.log")
        } else {
            agent.cfg.log_path.clone()
        };
        let mut tail = LogTail::new(&log_path);
        if tail.exists() {
            tail.skip_to_end();
            agent.attach_log(tail);
            println!("日志: {}", log_path.display());
        } else {
            println!("日志缺失（{}），无聊天感知；仍可用 F8/F9 控制", log_path.display());
        }
        agent.open_run_log();

        println!(
            "启动：hz={} mode={} 视角={} | F8 交还控制权 / F9 急停 / F10 切视角",
            agent.cfg.hz,
            agent.cfg.mode.as_str(),
            match agent.cfg.perspective_presses % 3 {
                0 => "第一",
                1 => "第三·背视",
                2 => "第三·前视",
                _ => unreachable!(),
            }
        );

        let dur = if seconds > 0 {
            Some(Duration::from_secs(seconds))
        } else {
            None
        };
        let hk = Hotkeys::new(&[
            agent.cfg.kill_switch_vk,
            agent.cfg.panic_vk,
            agent.cfg.toggle_attack_vk,
            agent.cfg.toggle_move_vk,
            agent.cfg.toggle_perspective_vk,
        ]);
        let stats = agent.run(Some(hk), dur);
        end_high_res_timer();

        println!("\n=== 结束 ===");
        println!("拍了 {} 次，用时 {:.2}s，实际 {:.1} Hz", stats.ticks, stats.elapsed.as_secs_f64(), stats.achieved_hz);
        println!(
            "阶段分布 off/idle/engage/retreat/recover = {:?}",
            stats.phase_hist
        );
        if stats.bg_pauses > 0 {
            println!("后台暂停 {} 次（窗口失焦时 AI 停手）", stats.bg_pauses);
        }
        println!("已强制松开所有按键。");
    }
}
