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
use trit_agent::perceive::{LogEvent, Sensor};
use trit_agent::trit::{from_balanced, to_balanced, trits_str, Tri};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let flags = Flags::parse(&args[1.min(args.len())..]);

    match cmd {
        "selftest" => selftest(&flags),
        "sim" => sim(&flags),
        "probe" => probe(&flags),
        "run" => run(&flags),
        "help" | "-h" | "--help" => help(),
        _ => {
            eprintln!("未知命令: {cmd}\n");
            help();
        }
    }
}

fn help() {
    println!(
        r#"trit-agent — 三值决策状态机 · Minecraft 角色托管

用法:
  trit-agent selftest [--hz 200] [--secs 2]   逻辑自检 + 高频节拍实测
  trit-agent sim [--verbose]                  离线场景回放
  trit-agent probe [--config path]            检查窗口与日志路径
  trit-agent run [--config path] [--seconds N] [--hz N] [--log path] [--mode hold|patrol|follow]
  trit-agent init                             生成默认配置文件

热键（运行中）:
  F8  开关 AI（关掉 = 立刻松手，控制权回你）
  F9  急停（同 F8 关闭，但额外强制松手一次）
  F6  开关「攻击」这一项次操作
  F7  开关「移动」这一项次操作

游戏内聊天指令（AI 开着时直接打字）:
  !ai on / !ai off      !status
  !mode hold|patrol|follow
  !attack off   !move off   !look off   !jump off   !use off
  !threat       !calm       !panic
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
            println!("  ok   {name}");
        } else {
            fail += 1;
            println!("  FAIL {name}");
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
        LogEvent::Sensor(Sensor {
            hp: 20.0,
            food: 17,
            x: 0.0,
            y: 64.0,
            z: 0.0,
            hostiles: 2,
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
    println!("mode = {}", cfg.mode.as_str());
    let mut a = Agent::new(cfg, Box::new(DryInput::new()));
    a.arm(true);

    let sensor = |hp: f32, hostiles: i32| {
        LogEvent::Sensor(Sensor {
            hp,
            food: 17,
            x: 0.0,
            y: 64.0,
            z: 0.0,
            hostiles,
        })
    };

    println!("tick  phase     move strafe yaw pitch atk jump use  reason");
    let mut last_phase = Phase::Off;
    let mut t: u64 = 0;
    while t <= 9_000 {
        match t {
            0 => a.ingest(sensor(20.0, 0), t),
            2_500 => a.ingest(sensor(20.0, 3), t),
            5_000 => a.ingest(sensor(6.0, 3), t),
            6_200 => a.ingest(LogEvent::Death("bfxh was slain by Zombie".into()), t),
            6_600 => a.ingest(sensor(20.0, 0), t),
            _ => {}
        }
        let d = a.step(t, 5);
        if d.phase != last_phase || verbose {
            let dirs = &d.dirs;
            println!(
                "{:<5} {:<9} {:^4} {:^6} {:^3} {:^5} {:^3} {:^4} {:^3}  {}",
                t,
                d.phase.name(),
                dirs[Channel::Forward.index()].ch(),
                dirs[Channel::Strafe.index()].ch(),
                dirs[Channel::Yaw.index()].ch(),
                dirs[Channel::Pitch.index()].ch(),
                dirs[Channel::Attack.index()].ch(),
                dirs[Channel::Jump.index()].ch(),
                dirs[Channel::Use.index()].ch(),
                d.reason.name()
            );
            last_phase = d.phase;
        }
        t += 100;
    }
    println!("\n阶段轨迹：idle → engage（3 个敌人）→ retreat（6 血）→ recover（死亡）→ idle");
    println!("注意 5000ms 那行：血低 + 有威胁 → 强制 Forward=-1（后退），这是阶段机覆盖裁决器的结果。");
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
                println!("窗口: 未找到标题含 \"{}\" 的窗口", cfg.window_title);
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

        let input = match Win32Input::attach(&cfg.window_title) {
            Some(i) => i,
            None => {
                eprintln!("找不到标题含 \"{}\" 的窗口。先启动游戏，或用 --window 指定关键字。", cfg.window_title);
                end_high_res_timer();
                std::process::exit(2);
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
            "启动：hz={} mode={} 第三视角={} | F8 开关 AI，F9 急停",
            agent.cfg.hz,
            agent.cfg.mode.as_str(),
            agent.cfg.third_person_on_start
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
        ]);
        let stats = agent.run(Some(hk), dur);
        end_high_res_timer();

        println!("\n=== 结束 ===");
        println!("拍了 {} 次，用时 {:.2}s，实际 {:.1} Hz", stats.ticks, stats.elapsed.as_secs_f64(), stats.achieved_hz);
        println!(
            "阶段分布 off/idle/engage/retreat/recover = {:?}",
            stats.phase_hist
        );
        println!("已强制松开所有按键。");
    }
}
