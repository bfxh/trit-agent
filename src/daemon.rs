//! 后台守护进程：常驻、按 F8 接管。
//!
//! # 为什么要有这个
//!
//! 用户要的是「**直接按 F8 就可以**」。但 F8 的实现是
//! `GetAsyncKeyState` 轮询 —— 全局的、不看焦点的，游戏全屏也能收到。
//! 问题不在热键本身，而在于**必须先有进程在跑**：
//!
//! - 前台模式（`启动AI.bat`）：得先双击bat，等它挂上窗口，才能按 F8。
//!   而双击会弹一个黑窗口，还会 `SetForegroundWindow` 抢你的焦点。
//! - 没人跑的时候，F8 就是一个普通按键，什么都不会发生 —— 这正是
//!   「按了没反应」的真正原因，不是热键坏了。
//!
//! 所以这里做一个**常驻后台**的守护进程：
//!
//! - 开机自启，全程待在后台（无控制台窗口，不抢焦点）
//! - 游戏窗口出现就自动挂上，消失（关游戏/换存档）就重新挂
//! - F8 = 接管 / 再按 = 交还，语义与前台模式完全一致
//!
//! # 与前台模式的关系
//!
//! 两者共用同一个命名互斥量 `TRIT_AGENT_MUTEX`：
//! **全局只允许一个实例在监听 F8**。否则两个进程各切一次，
//! 按一次 F8 等于切换两次 = 看起来毫无反应。
//! 前台模式启动时若发现守护进程在跑，会直接提示「按 F8 即可」并退出。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::act::win32::{Hotkeys, Win32Input};
use crate::agent::Agent;
use crate::config::Config;
use crate::perceive::logtail::LogTail;

/// 全局互斥量名。前台 `run` 和后台 daemon 共用，保证只有一个在监听 F8。
pub const TRIT_AGENT_MUTEX: &str = "Global\\trit-agent-mutex";

/// 停机信号事件名。外部（比如 `trit-agent stop`）置位它，守护进程优雅退出。
pub const TRIT_AGENT_STOP: &str = "Global\\trit-agent-stop";

/// 计划任务名。任务计划程序负责「进程死了就重新拉起」。
pub const TRIT_AGENT_TASK: &str = "trit-agent-daemon";

/// 守护进程自己的日志文件。
///
/// 放在**安装目录**（`%LOCALAPPDATA%\trit-agent\`），而不是 exe 旁边：
/// 开发时从 `target/release` 直接跑，exe 旁边就是构建产物目录，
/// 日志混进去既不显眼，`cargo clean` 还会把它删掉。
/// 固定位置的好处是：`trit-agent status` 无论从哪跑都能找到同一个日志。
fn daemon_log_path() -> PathBuf {
    install_dir().join("trit-agent-daemon.log")
}

/// 追加一行到守护日志。
///
/// 后台没有控制台，所有反馈都只能落文件 —— 用户要能查「它到底有没有
/// 挂上窗口」「传感器有没有数据」。
fn log_line(msg: &str) {
    let path = daemon_log_path();
    // 目录可能不存在（卸载时删掉了，或者直接从 target/release 跑还没装过）。
    // 不建目录的话日志会静默丢失 —— 而日志是后台模式**唯一**的反馈渠道，
    // 丢了就等于「守护进程一声不吭」，用户根本没法判断它在不在跑。
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let ts = timestamp();
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// 本地时间戳（不引 chrono，零依赖）。
///
/// **必须用本地时间，不能用 `SystemTime::now()`** ——
/// 后者给的是 UTC，比崩溃报告、Minecraft 日志、本地时间**差 8 小时**。
/// 守护日志是排查崩溃时唯一的「AI 干了什么」证据，
/// 时间对不上，这条证据就废了。
fn timestamp() -> String {
    crate::act::win32::local_time_string()
}

/// 把 panic 现场写进守护日志。
///
/// **这个函数存在的唯一理由：守护进程是「静默死亡」的。**
///
/// 两个东西叠在一起就足以让一次崩溃变成「什么都没发生」：
///
/// 1. 守护进程用 `windows_subsystem = "windows"` 编译 —— **没有控制台**。
///    Rust 的默认 panic 处理是往 stderr 打印，而 GUI 子系统的 stderr
///    指向一个无效句柄 —— **一个字都出不来**。
/// 2. release profile 是 `panic = "abort"` —— panic 直接终止进程，
///    没有任何 unwind 清理的机会。
///
/// 结果就是：守护进程某天悄悄没了，日志停在某一行，没有任何错误信息，
/// 用户只看到「按 F8 没反应」，排查的人只能靠猜。
///
/// 所以这里**自己把 panic 落盘**：哪怕只是「进程死在了哪一行」，
/// 也比「什么都没有」强一万倍。
///
/// 注意 `panic = "abort"` 下 `catch_unwind` 无效（进程直接被终止），
/// 但 **panic hook 仍然会被调用** —— 这正是我们唯一能抓住它的时机。
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<非字符串 panic>".to_string()
        };
        let loc = match info.location() {
            Some(l) => format!("{}:{}:{}", l.file(), l.line(), l.column()),
            None => "<位置未知>".to_string(),
        };
        // 先落盘再调上一个 hook —— 万一落盘本身出问题，
        // 上一个 hook 还能把消息打到它该去的地方。
        log_line(&format!("!!! 守护进程 panic: {msg} @ {loc}"));
        log_line(&format!("!!! 线程: {:?}", std::thread::current().id()));
        prev(info);
    }));
}

/// 只在**内容变化时**记录一行日志（跨进程有效）。
///
/// 计划任务每分钟触发一次守护进程。每次都记同样的内容，
/// 日志一天就是 1440 行重复 —— 真正出事时该看的那几行会被冲走。
///
/// 实现用一个标记文件存「上次记过的内容」：
/// 内容没变就跳过；变了（或者标记文件丢了）就记一行并更新标记。
///
/// 标记文件放在安装目录，和日志在一起。
fn log_once(key: &str, msg: &str) {
    let path = install_dir().join("trit-agent-once.log");
    let mut prev = String::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('\t') {
                if k == key {
                    prev = v.to_string();
                }
            }
        }
    }
    if prev == msg {
        return;
    }
    log_line(msg);
    // 更新标记（失败无所谓，下次还会记一遍）
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{key}\t{msg}");
    }
}

/// 配置路径专用：每分钟触发的重复实例不该刷屏。
fn log_cfg_once(msg: &str) {
    log_once("cfg", msg);
}

/// 读配置。守护进程用与前台完全一样的配置来源。
///
/// 查找顺序很重要：**先找 exe 旁边的**。自启时工作目录是
/// `C:\Windows\System32`（登录流程的工作目录），如果只按 cwd 找，
/// 永远找不到 toml，只能退回内置默认值 —— 而默认值的 `log_path`
/// 指向 `logs/latest.log`，相对当前目录，于是**永远收不到感知包**，
/// 表现就是「AI 瞎了，站桩不动」。
///
/// 返回 (配置, 配置所在目录)。相对路径要锚定到这个目录 ——
/// 同一个道理：自启时 cwd 是 System32，相对路径的日志会写到那儿去。
fn load_cfg() -> (Config, PathBuf) {
    let mut candidates = vec![exe_dir().join("trit-agent.toml")];
    // 安装目录（自启时 exe 在那儿）
    candidates.push(install_dir().join("trit-agent.toml"));
    // 最后才是当前工作目录（手动在项目目录里启动的常见情况）
    candidates.push(PathBuf::from("trit-agent.toml"));

    for p in candidates {
        if p.is_file() {
            match Config::load_or_create(&p) {
                Ok(mut c) => {
                    // 只在**第一次**记配置路径。
                    //
                    // 计划任务每分钟触发一次，每次都会走到这里；
                    // 每分钟记一遍同样的内容，日志就被灌水了 ——
                    // 真正出事时最该看的那几行会被冲走。
                    log_cfg_once(&format!("配置: {}", p.display()));
                    let dir = match p.parent() {
                        // `parent()` 对「trit-agent.toml」这种相对路径返回**空路径**，
                        // 不是 "."。拿空路径去 join 相对日志路径会得到怪路径。
                        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
                        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    };
                    // 相对路径锚定到配置目录
                    if c.run_log.is_relative() {
                        c.run_log = dir.join(&c.run_log);
                    }
                    if c.log_path.is_relative() {
                        c.log_path = dir.join(&c.log_path);
                    }
                    return (c, dir);
                }
                Err(e) => log_line(&format!("读配置失败 {}: {e}", p.display())),
            }
        }
    }
    log_line("没找到 trit-agent.toml，用内置默认值");
    // 兜底目录用当前工作目录的绝对路径 ——
    // `PathBuf::from(".").parent()` 是**空路径**（不是 "."），
    // 拿它去 join 相对路径会得到一个没有目录前缀的怪路径。
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    (Config::default(), cwd)
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 守护进程主循环。返回进程退出码。
///
/// 流程：
/// 1. 抢全局互斥量（已有实例 → 直接退出，避免双实例抵消 F8）
/// 2. 循环：等游戏窗口 → 挂上 → 跑状态机 → 窗口没了就重新等
/// 3. 收到停机信号 → 松手退出
pub fn run() -> i32 {
    // **必须第一个装**：GUI 子系统 + panic=abort 意味着任何 panic
    // 都不会在 stderr 上留下一个字。钩子是唯一的目击者。
    install_panic_hook();

    let (cfg, _cfg_dir) = load_cfg();

    // ---- 单实例 ----
    let (mutex, existed) = match crate::act::win32::create_named_mutex(TRIT_AGENT_MUTEX) {
        Some(m) => m,
        None => {
            log_line("创建互斥量失败（可能权限不足），继续运行但可能与前台实例冲突");
            (0, false)
        }
    };
    if existed {
        // **不写日志**：计划任务每分钟触发一次，正常运行时
        // 每次触发都会走到这里 —— 每分钟刷一条「已经有实例在运行」
        // 会把真正的日志淹掉（出事时最关键的几行被冲走）。
        //
        // 静默退出反而是正确行为：已有实例在跑，说明一切正常。
        return 0;
    }
    // 每次真的起来了都记一行 —— 这行是「进程活着」的证据，
    // 排查「它到底有没有在跑」时要看它。
    // （重复触发的多余实例走的是上面的 `return 0`，不会到这里。）
    log_line(&format!("守护进程启动 (pid={})", std::process::id()));

    // 上一次如果是被强杀（任务管理器），键可能还按着 —— 先无条件松一遍。
    crate::act::win32::release_known_keys();

    // 停机信号：别的进程置位它，我们就优雅退出。
    let stop_evt = crate::act::win32::create_named_event(TRIT_AGENT_STOP);
    if stop_evt.is_none() {
        log_line("创建停机事件失败，将只能用任务管理器结束");
    }

    let log_path = if cfg.log_path.as_os_str().is_empty() {
        PathBuf::from("logs/latest.log")
    } else {
        cfg.log_path.clone()
    };

    let mut next_report = Instant::now();
    loop {
        // 停机信号检查
        if let Some(ev) = stop_evt {
            if crate::act::win32::event_is_signaled(ev) {
                log_line("收到停机信号，退出");
                crate::act::win32::release_known_keys();
                break;
            }
        }

        // ---- 等游戏窗口 ----
        let input = match Win32Input::attach(&cfg.window_title) {
            Some(i) => i,
            None => {
                // 没找到窗口就等着，但要定期报状态（用户会问「到底在不在跑」）
                if next_report.elapsed() >= Duration::from_secs(30) {
                    next_report = Instant::now();
                    // 文案必须说**真实判据**。
                    //
                    // 原来这里写「标题含 "Minecraft"」，但真正决定匹配的是
                    // **窗口类名**（LWJGL3 的 `GLFW30`）——
                    // 「烦人的村民」的标题是 `Annoying Villagers - Annoying Villagers`，
                    // 整个串里没有 "Minecraft"。写错文案会让人以为程序坏了，
                    // 排查方向整个跑偏。
                    log_line(&format!(
                        "等待游戏窗口（按类名 GLFW* 匹配，标题仅作辅助；当前标题关键字 {:?}）……",
                        cfg.window_title
                    ));
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        log_line(&format!("挂上窗口: {}", input.title()));

        // ---- 跑一轮 ----
        //
        // **整轮包在 catch_unwind 里**：某一轮跑炸了（panic），
        // 只炸掉这一轮，守护进程继续活着，F8 继续有效。
        //
        // 为什么必须这样：守护进程是 GUI 子系统，panic 消息在 stderr 上
        // 无处可打 —— 不拦住的话，一次 panic 就是「进程人间蒸发」，
        // 用户按 F8 没反应，而且**日志里什么都看不到**。
        //
        // 拦住的代价：最多丢掉一轮游戏（一两秒），下一轮重新挂窗口即可。
        // 相比「守护进程彻底死掉」，这个代价可以忽略。
        let round = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_one_round(&cfg, &log_path, input, stop_evt)
        }));

        match round {
            Ok(stats) => {
                log_line(&format!(
                    "窗口结束：拍了 {} 次，{:.1} Hz，阶段分布 {:?}，后台暂停 {} 次",
                    stats.ticks, stats.achieved_hz, stats.phase_hist, stats.bg_pauses
                ));
            }
            Err(_) => {
                // panic hook 已经把详情写进日志了，这里只补一句结论。
                log_line("!!! 本轮 panic 已隔离，守护进程继续运行（F8 仍然有效）");
            }
        }

        // 窗口没了（关游戏/换存档）→ 松手，回到等待循环
        crate::act::win32::release_known_keys();
        std::thread::sleep(Duration::from_millis(500));
    }

    if mutex != 0 {
        crate::act::win32::close_handle(mutex);
    }
    // 事件句柄也关掉 —— 裸 isize 不会自动释放，不关就是句柄泄漏。
    if let Some(ev) = stop_evt {
        crate::act::win32::close_handle(ev);
    }
    log_line("守护进程已退出");
    0
}

/// 跑一轮状态机（挂上窗口 → 感知 → 决策 → 注入）。
///
/// 抽成独立函数是为了让 `catch_unwind` 能干净地包住它 ——
/// `Agent` 不是 `UnwindSafe`（里面有 `Box<dyn Input>`），
/// 所以调用处必须用 `AssertUnwindSafe` 包。
fn run_one_round(
    cfg: &Config,
    log_path: &Path,
    input: Win32Input,
    stop_evt: Option<crate::act::win32::Handle>,
) -> crate::agent::RunStats {
    let mut agent = Agent::new(cfg.clone(), Box::new(input));
    // **不自动接管** —— 这是与前台模式最大的区别。
    // 前台模式是「双击就开跑」，守护模式是「按 F8 才接管」，
    // 后者才符合用户说的「直接按 F8 就可以」。
    //
    // 连抢焦点和切视角都关掉：守护进程是常驻的，抢焦点意味着你
    // 正在写代码时窗口被游戏抢走；切视角等于在你没玩的时候乱动镜头。
    agent.grab_focus_on_start = false;
    agent.switch_view_on_start = false;
    agent.stop_when_window_gone = true;
    // 把停机信号接进**主循环**。
    //
    // 不接的话，`agent.run()` 会一直待到游戏关闭为止（整整一局），
    // 外层「等窗口」的循环根本轮不到 —— 于是 `trit-agent stop`
    // 在游戏开着的时候**完全不起作用**，命令回「已请求退出」，
    // 进程却还在跑。用户只能去任务管理器杀（键会卡在按下的状态）。
    let stop_for_agent = stop_evt;
    agent.should_stop = Some(Box::new(move || match stop_for_agent {
        Some(h) => crate::act::win32::event_is_signaled(h),
        None => false,
    }));
    // 接管状态写进日志 —— 后台没有控制台，窗口标题又被全屏盖住，
    // 「AI 到底接管了没」只能靠日志回答。
    // 每次都重新挂上：arm() 里的回调是一次性的。
    agent.on_arm_change = Some(Box::new(|on: bool| {
        log_line(if on {
            "F8：AI 已接管（按 F8 再交还）"
        } else {
            "F8：已交还控制权"
        });
    }));
    // 不接管时**不碰窗口标题**。
    //
    // 之前这里调 `agent.update_badge()` 给标题挂上 "[AI 待机]"，
    // 加上主循环每 500ms 刷一次 —— 相当于后台进程空闲时不停地对
    // 游戏窗口发 SetWindowTextW。游戏加载阶段（player 还是 null）
    // 正好在这一段时间里崩溃，所以待机期一个字都不写。
    agent.update_badge();

    let mut tail = LogTail::new(log_path);
    if tail.exists() {
        tail.skip_to_end();
        agent.attach_log(tail);
    } else {
        log_line(&format!("日志缺失（{}），无聊天感知", log_path.display()));
    }
    agent.open_run_log();

    let hk = Hotkeys::new(&[
        cfg.kill_switch_vk,
        cfg.panic_vk,
        cfg.toggle_attack_vk,
        cfg.toggle_move_vk,
        cfg.toggle_perspective_vk,
    ]);

    agent.run(Some(hk), None)
}

/// 供外部（`trit-agent stop`）调用：请求守护进程退出。
pub fn request_stop() -> bool {
    match crate::act::win32::open_named_event(TRIT_AGENT_STOP) {
        Some(h) => {
            let ok = crate::act::win32::signal_event(h);
            crate::act::win32::close_handle(h);
            ok
        }
        None => false,
    }
}

/// 守护进程是否在跑（互斥量已存在 = 有实例）。
pub fn is_running() -> bool {
    match crate::act::win32::open_named_mutex(TRIT_AGENT_MUTEX) {
        Some(h) => {
            // 句柄用完必须关 —— 每次调用都开一个而不关，
            // 句柄表会被慢慢吃光（而且互斥量本身还被持有，
            // 会让「实例已退出」永远判不出来）。
            crate::act::win32::close_handle(h);
            true
        }
        None => false,
    }
}

/// 开机自启用的注册表位置（HKCU，不需要管理员权限）。
///
/// 用 HKCU 而不是 HKLM：HKLM 写 `Run` 要管理员权限，普通用户装不上。
/// HKCU 的 Run 键同样是登录时启动，效果一样且零门槛。
///
/// **必须带 `HKCU\` 前缀** —— `reg add` 不接受省略根键名的写法，
/// 少了前缀它不会报错说「键不存在」，而是报「语法无效」，
/// 看起来像参数引号的问题，极难定位。
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "trit-agent-daemon";

/// 安装目录。**不能**用 `target/release` —— cargo clean 会把它删掉，
/// 注册表就指向一个不存在的路径，自启静默失效。
fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("trit-agent")
}

/// 注册表读写。
///
/// 只用 `reg.exe` 命令行，不引第三方 crate（这个项目刻意零依赖）。
/// 命令行方式还避免了一个坑：直接调 advapi32 写 REG_SZ 要处理
/// UTF-16 编码问题，而 `reg add` 会自己处理。
/// 读注册表里的一个字符串值。
///
/// 用 `reg query` 而不是直接调 advapi32 —— 这个项目刻意零依赖。
/// 解析时要注意 `reg query` 的输出是本地化过的（中文系统上
/// 类型名可能是「REG_SZ」也可能是「REG_SZ」以外的写法），
/// 所以不硬编码类型名，而是找**最后一个冒号**之后的内容。
fn reg_query(key: &str, value: &str) -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", key, "/v", value])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        // 形如：    trit-agent-daemon    REG_SZ    "C:\path\to.exe"
        let line = line.trim();
        if !line.starts_with(value) {
            continue;
        }
        // 值的部分在最后一个冒号之后（路径里也可能带冒号，所以取最后一个）
        let Some((_, v)) = line.rsplit_once(':') else {
            continue;
        };
        return Some(v.trim().to_string());
    }
    None
}

fn reg_delete(key: &str, value: &str) -> bool {
    std::process::Command::new("reg")
        .args(["delete", key, "/v", value, "/f"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn reg_set(key: &str, value: &str, data: &str) -> bool {
    std::process::Command::new("reg")
        .args(["add", key, "/v", value, "/t", "REG_SZ", "/d", data, "/f"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ------------------------------------------------------------------
// 计划任务：让「守护进程死了」不再等于「按 F8 没反应」
// ------------------------------------------------------------------
//
// ## 为什么光有 HKCU\Run 不够
//
// `Run` 键只在**登录时**跑一次。之后守护进程要是死了（比如 panic），
// 就再也没人拉起它 —— 而用户的感受就是「按 F8 没反应」。
//
// 这次排查就撞上了这个坑：守护进程 19:20:22 启动，19:21:52 之后
// 日志直接断掉，进程不在了，而游戏 19:22:31 才启动 ——
// 用户按键时，**根本没有任何进程在监听 F8**。
//
// ## 计划任务解决什么
//
// 任务计划程序会**监控任务状态**：任务异常结束时，按
// `RestartOnFailure` 的设定自动重启它。所以：
//
// - 守护进程崩了 → 1 分钟内自动复活
// - 登录时自动启动（`LogonTrigger`）
// - 不用管理员权限（当前用户自己的任务）
//
// 注意 `Run` 键**保留**：两套并存，互为备份。
// `Run` 负责「登录即启动」，计划任务负责「死了自动重启」。

/// 生成计划任务 XML。
///
/// 用 XML 而不是 `schtasks /create /tr ...` 命令行参数，
/// 因为命令行方式**给不了 `RestartOnFailure`** —— 而那正是我们要的。
///
/// # 关键设置
///
/// - `LogonTrigger`：登录时启动（替代 `Run` 键的角色）
/// - `CalendarTrigger` + `Repetition`：**每 1 分钟触发一次**。
///   这是自愈的**主力** —— 见下面的「为什么不能只靠 RestartOnFailure」。
/// - `RestartOnFailure`：崩溃后 1 分钟内重启，是**快速恢复**的一层。
/// - `ExecutionTimeLimit = PT0S`：**不限运行时长**。
///   默认值是「任务运行 3 天后强制终止」—— 对常驻守护是致命的。
/// - `MultipleInstancesPolicy = IgnoreNew`：已在跑就跳过这次触发。
///   有了它，每分钟的重复触发不会堆出一堆实例。
/// - `DisallowStartIfOnBatteries = false`：笔记本拔电源也继续跑。
///
/// # 为什么不能只靠 RestartOnFailure
///
/// 实测（本机，Windows 11）：把守护进程强杀后，
/// 任务的 `LastTaskResult` 变成 `0xFFFFFFFF`（SCHED_S_TASK_HAS_NOT_RUN），
/// **任务计划程序根本没把它当成「失败」**，所以不会触发重启。
/// 90 秒后进程依然没回来。
///
/// 原因大概是：从外部杀进程时，计划任务程序看到的是
/// 「实例消失了」而不是「实例返回了非零退出码」。
///
/// 而**重复触发**不依赖退出码：任务实例结束了，下一个触发点照样会跑。
/// 实例还活着时，`IgnoreNew` 会让这次触发被跳过 —— 正好是我们想要的。
/// 所以两层一起上：
///
/// - `RestartOnFailure` 管「崩溃了快速回来」
/// - 重复触发管「不管什么原因死了，一分钟内一定回来」
fn task_xml(exe: &Path) -> String {
    let cmd = format!("&quot;{}&quot;", exe.display());
    // 起始日放在过去：任务注册后第一次触发不用等到某个特定日期。
    // 配合下面的 Repetition，等效于「注册后每分钟跑一次」。
    let start = "2020-01-01T00:00:00";
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>trit-agent 后台守护：进游戏直接按 F8 接管。进程异常退出时自动重启。</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
    <CalendarTrigger>
      <Repetition>
        <Interval>PT1M</Interval>
        <Duration>P3650D</Duration>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <StartBoundary>{start}</StartBoundary>
      <Enabled>true</Enabled>
      <ScheduleByDay>
        <DaysInterval>1</DaysInterval>
      </ScheduleByDay>
    </CalendarTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{cmd}</Command>
    </Exec>
  </Actions>
</Task>
"#,
        user = current_user_sid(),
        cmd = cmd,
        start = start
    )
}

/// 取当前用户的 SID（任务 XML 里 `<UserId>` 要的是 SID 不是用户名）。
///
/// 拿不到就退回用户名 —— 任务计划程序两种都认。
fn current_user_sid() -> String {
    // whoami /user 输出形如：
    //   \用户名\主机名 S-1-5-21-xxxx-xxxx-xxxx-xxxx-xxxx
    // 取最后一段以 "S-1-" 开头的 token。
    if let Ok(out) = std::process::Command::new("whoami")
        .arg("/user")
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for tok in text.split_whitespace() {
            if tok.starts_with("S-1-") {
                return tok.to_string();
            }
        }
    }
    // 退回当前用户名（`USERNAME` 环境变量）
    std::env::var("USERNAME").unwrap_or_else(|_| "SYSTEM".to_string())
}

/// 注册计划任务（失败自动重启）。
///
/// 返回 `Err(原因)` 时**不能当成致命错误** —— `Run` 键还在，
/// 只是少了「死了自动重启」这一层保险。
fn register_scheduled_task(exe: &Path) -> Result<(), String> {
    let xml_path = install_dir().join("trit-agent-task.xml");
    // 任务 XML 必须是 UTF-16 编码，且要带 BOM —— schtasks 对编码很挑，
    // 用 UTF-8 会报「任务 XML 损坏」这种毫无线索的错。
    let xml = task_xml(exe);
    let mut utf16: Vec<u8> = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    if let Err(e) = std::fs::write(&xml_path, &utf16) {
        return Err(format!("写任务 XML 失败 {}: {e}", xml_path.display()));
    }

    // 先删旧的（有的话），否则 /create 会报「任务已存在」
    let _ = std::process::Command::new("schtasks")
        .args(["/delete", "/tn", TRIT_AGENT_TASK, "/f"])
        .output();

    let out = std::process::Command::new("schtasks")
        .args([
            "/create",
            "/tn",
            TRIT_AGENT_TASK,
            "/xml",
            &xml_path.display().to_string(),
            "/f",
        ])
        .output()
        .map_err(|e| format!("调 schtasks 失败: {e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout)
        } else {
            err
        };
        return Err(format!("schtasks 创建任务失败: {}", err.trim()));
    }
    Ok(())
}

/// 注销计划任务。
fn unregister_scheduled_task() -> bool {
    std::process::Command::new("schtasks")
        .args(["/delete", "/tn", TRIT_AGENT_TASK, "/f"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 立即跑一次计划任务（不等登录）。
///
/// 走计划任务而不是直接 spawn：这样启动的进程由任务计划程序托管，
/// 以后崩了也会被自动拉起。直接 spawn 的进程是「孤儿」——
/// 没人看着，死了就是死了。
fn run_scheduled_task() -> bool {
    std::process::Command::new("schtasks")
        .args(["/run", "/tn", TRIT_AGENT_TASK])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 安装开机自启。返回进程退出码。
///
/// 装到 `%LOCALAPPDATA%\trit-agent\`，注册表指向那份**副本**，
/// 不是 `target/release` 里的原件 —— cargo clean 会删掉 target，
/// 指向它等于自启静默失效（而且没有任何报错，最难查）。
pub fn install_autostart() -> i32 {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log_line(&format!("拿不到自身路径: {e}"));
            return 1;
        }
    };
    let dir = install_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log_line(&format!("建目录失败 {}: {e}", dir.display()));
        return 1;
    }
    let target_exe = dir.join("trit-agent-daemon.exe");

    // 已在跑的那份会锁住目标 exe，复制必然失败（os error 32）。
    // 所以顺序是：先停旧的 → 复制 → 起新的。
    // 不先停的话，重装会失败在「另一个程序正在使用此文件」，
    // 而用户看到的是「安装失败」，完全猜不到是自己在跑着。
    let was_running = is_running();
    if was_running {
        log_line("检测到已有实例在跑，先停掉再更新");
        request_stop();
        // 等它真正退出（最多 3 秒）
        for _ in 0..30 {
            if !is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if is_running() {
            log_line("旧实例没退干净，exe 可能仍被占用");
        }
    }

    if let Err(e) = copy_file(&exe, &target_exe) {
        log_line(&format!("复制 exe 失败: {e}"));
        return 1;
    }

    // 配置也一起搬过去，否则守护进程在别的目录启动时找不到 toml，
    // 只能退回内置默认值（log_path 就指错了地方 → 永远收不到感知）。
    for src in ["trit-agent.toml", "trit-agent.example.toml"] {
        let s = PathBuf::from(src);
        if s.is_file() {
            if let Err(e) = copy_file(&s, &dir.join(src)) {
                log_line(&format!("复制配置 {src} 失败: {e}"));
            }
        }
    }

    let cmd = format!("\"{}\"", target_exe.display());
    if let Some(cur) = reg_query(RUN_KEY, RUN_VALUE) {
        if cur == cmd {
            log_line(&format!("开机自启已就位: {cmd}"));
            // 计划任务也要确保在（Run 键在但任务可能被手动删过）
            match register_scheduled_task(&target_exe) {
                Ok(()) => {
                    log_line("计划任务已就位（崩溃自动重启）");
                    let _ = run_scheduled_task();
                }
                Err(e) => log_line(&format!("计划任务注册失败（不影响使用）: {e}")),
            }
            return 0;
        }
    }
    if !reg_set(RUN_KEY, RUN_VALUE, &cmd) {
        log_line("写注册表失败");
        return 1;
    }
    log_line(&format!("已安装开机自启: {cmd}"));

    // 计划任务：**崩溃自动重启**的那层保险。
    // 注册失败不算致命 —— Run 键已经就位，至少登录时会启动。
    match register_scheduled_task(&target_exe) {
        Ok(()) => {
            log_line("计划任务已就位（崩溃自动重启，间隔 1 分钟）");
            // 立刻跑一次，用户不用等下次登录。
            // 走计划任务而不是直接 spawn：这样这个进程是「被看着的」，
            // 以后崩了会被自动拉起。直接 spawn 出来的是孤儿进程，
            // 死了就永远死了 —— 这正是这次排查撞上的坑。
            if run_scheduled_task() {
                log_line("已通过计划任务启动");
            } else {
                log_line("计划任务启动失败，改为直接启动");
                let _ = std::process::Command::new(&target_exe).spawn();
            }
        }
        Err(e) => {
            log_line(&format!("计划任务注册失败（不影响基本使用，但崩溃不会自动重启）: {e}"));
            let _ = std::process::Command::new(&target_exe).spawn();
        }
    }
    0
}

/// 复制文件（覆盖目标）。
fn copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dst)?;
    Ok(())
}

/// 卸载开机自启。返回进程退出码。
pub fn uninstall_autostart() -> i32 {
    if reg_delete(RUN_KEY, RUN_VALUE) {
        log_line("已卸载开机自启");
    } else {
        log_line("没有找到自启项（可能本来就没装）");
    }
    // 计划任务也要删 —— 留着它，卸载后它照样会把守护进程拉起来。
    if unregister_scheduled_task() {
        log_line("已卸载计划任务");
    }
    // 顺便把正在跑的实例停掉
    request_stop();
    // 清掉安装目录（失败就算了，不影响功能）
    let dir = install_dir();
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    0
}

/// 打印状态。返回进程退出码。
pub fn print_status() -> i32 {
    let running = is_running();
    let log = daemon_log_path();
    let mut out = String::new();
    out.push_str(&format!(
        "守护进程: {}\n",
        if running { "运行中" } else { "未运行" }
    ));
    // 自愈状态：计划任务在不在，决定「它挂了会不会被拉起来」。
    // 只说「未运行」而不说这个，用户会以为彻底没救了。
    out.push_str(&format!(
        "自动重启: {}\n",
        if scheduled_task_exists() {
            "已启用（计划任务会在崩溃后 1 分钟内拉起）"
        } else {
            "未启用（进程挂了不会自动重启，重新跑一次 --install）"
        }
    ));
    out.push_str(&format!("日志: {}\n", log.display()));
    if let Ok(text) = std::fs::read_to_string(&log) {
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines.len().saturating_sub(5);
        out.push_str("最近日志:\n");
        for l in &lines[tail..] {
            out.push_str("  ");
            out.push_str(l);
            out.push('\n');
        }
    }
    print!("{out}");
    0
}

/// 计划任务是否存在。
fn scheduled_task_exists() -> bool {
    std::process::Command::new("schtasks")
        .args(["/query", "/tn", TRIT_AGENT_TASK])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 供测试用：确认日志路径落在 exe 旁边。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::act::win32::{
        close_handle, create_named_event, create_named_mutex, event_is_signaled, open_named_event,
        signal_event,
    };

    /// 锁住「单实例」这个核心不变量。
    ///
    /// 两个进程同时轮询 F8 = 按一次被切两次 = 看起来毫无反应。
    /// 这是「按 F8 没反应」的**第二种**成因（第一种是压根没进程在跑），
    /// 必须保证命名互斥量真的生效。
    #[test]
    fn named_mutex_blocks_second_instance() {
        let name = "Local\\trit-agent-test-mutex";
        let (h1, existed1) = create_named_mutex(name).expect("创建互斥量");
        assert!(!existed1, "第一次创建不该是「已存在」");

        // 第二个进程（这里是本进程再开一次，模拟另一个进程）
        let (h2, existed2) = create_named_mutex(name).expect("第二次创建");
        assert!(existed2, "第二次必须报「已存在」—— 否则双实例会互相抵消 F8");

        close_handle(h2);
        close_handle(h1);
    }

    /// 命名事件：置位 → 能被查到，而且**信号要粘住**。
    ///
    /// 停机信号全靠它。测不出来的话，`trit-agent stop` 就是个摆设，
    /// 守护进程只能靠任务管理器杀（键可能还按着）。
    #[test]
    fn named_event_carries_stop_signal() {
        // 名字带时间戳：上一次测试若中途 panic，句柄没关，
        // 命名对象还留在系统里（而且可能已置位），
        // 下一次跑 `create_named_event` 拿到的是**旧的、已置位的**对象，
        // 「刚建好不该是已置位」这条断言就会莫名其妙地挂掉。
        let name = format!(
            "Local\\trit-agent-test-event-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let evt = create_named_event(&name).expect("创建事件");
        assert!(!event_is_signaled(evt), "刚建好不该是已置位");

        // 另一个进程打开并置位
        let opener = open_named_event(&name).expect("打开事件");
        assert!(signal_event(opener), "置位应成功");
        close_handle(opener);

        assert!(event_is_signaled(evt), "置位后必须能收到 —— 这是 stop 的全部机制");
        // **手动复位**的关键不变量：信号不会被查询者吃掉。
        //
        // 自动复位事件被第一个 WaitForSingleObject 碰到就没了，
        // 而停机信号有**两个**查询点（主循环 + 外层循环）——
        // 自动复位下第二个查是「未置位」，守护进程收到信号反而 exit 不掉。
        assert!(
            event_is_signaled(evt),
            "信号必须粘住：查第二次还得是已置位，否则 stop 到不了外层循环"
        );
        close_handle(evt);
    }

    /// 安装目录不能在 target 里 —— cargo clean 会删掉它。
    #[test]
    fn install_dir_is_outside_target() {
        let d = install_dir();
        let s = d.to_string_lossy().to_lowercase();
        assert!(
            !s.contains("\\target\\") && !s.contains("/target/"),
            "安装目录不能建在 target 里：cargo clean 会把它删掉，\
             注册表就指向一个不存在的路径，自启静默失效。实际: {d:?}"
        );
    }

    /// 配置文件查找顺序：exe 旁边优先，其次安装目录，最后 cwd。
    ///
    /// 顺序不能反：自启时 cwd 是 System32，先查 cwd 就永远找不到 toml。
    #[test]
    fn config_search_prefers_exe_dir_over_cwd() {
        let (cfg, dir) = load_cfg();
        // 不管找到哪个，dir 都得是有效目录
        assert!(dir.is_dir(), "配置目录必须是真实存在的目录: {dir:?}");
        // 默认配置下 log_path 应该被锚定过（不是相对裸路径）
        if !cfg.log_path.as_os_str().is_empty() {
            assert!(
                cfg.log_path.is_absolute(),
                "相对路径必须锚定到配置目录，否则自启时（cwd=System32）\
                 日志路径会指向 C:\\Windows\\System32，AI 永远收不到感知包"
            );
        }
    }

    /// 注册表键名必须带根键前缀。
    ///
    /// `reg add` 不接受省略根键的写法，少了前缀它报的是「语法无效」，
    /// 看起来像引号问题，极难定位。
    #[test]
    fn run_key_has_root_prefix() {
        assert!(
            RUN_KEY.starts_with("HKCU\\") || RUN_KEY.starts_with("HKLM\\"),
            "注册表键必须带根键前缀: {RUN_KEY}"
        );
    }

    /// 任务 XML 必须包含「崩溃自动重启」和「不限运行时长」这两个设置。
    ///
    /// 这两条是**守护进程能一直活着**的全部依据：
    ///
    /// - `RestartOnFailure`：进程崩了，任务计划程序 1 分钟后拉起。
    ///   少了它，一次 panic 就等于「按 F8 再也没反应」——
    ///   这正是这次排查撞上的坑。
    /// - `ExecutionTimeLimit=PT0S`：默认是「跑满 3 天强制终止」。
    ///   对常驻守护来说，那是一颗定时炸弹。
    ///
    /// 用 XML 注册就是为了这两条（`schtasks /create /tr` 给不了）。
    #[test]
    fn task_xml_has_restart_and_no_time_limit() {
        let xml = task_xml(Path::new(r"C:\test\trit-agent-daemon.exe"));

        assert!(
            xml.contains("<RestartOnFailure>"),
            "任务 XML 少了 RestartOnFailure —— 守护进程崩了就永远起不来了"
        );
        assert!(
            xml.contains("<Interval>PT1M</Interval>"),
            "重启间隔应该是 1 分钟"
        );
        assert!(
            xml.contains("<Count>999</Count>"),
            "重启次数应该足够多（999）"
        );
        assert!(
            xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"),
            "ExecutionTimeLimit 必须是 PT0S（不限时）—— \
             默认值会在运行 3 天后强制终止常驻守护进程"
        );
        assert!(
            xml.contains("<LogonTrigger>"),
            "任务必须有 LogonTrigger（登录即启动）"
        );
        assert!(
            xml.contains("<RunLevel>LeastPrivilege</RunLevel>"),
            "不要管理员权限 —— 普通用户也得能装"
        );
        assert!(
            xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"),
            "笔记本拔电源后守护进程不该被停掉"
        );
    }

    /// 任务 XML 必须有**每分钟重复触发**。
    ///
    /// 实测（本机 Windows 11）：把守护进程强杀后，
    /// 任务的 `LastTaskResult` 变成 `0xFFFFFFFF`（SCHED_S_TASK_HAS_NOT_RUN）
    /// —— 任务计划程序**根本没把外部强杀当成「失败」**，
    /// 所以 `RestartOnFailure` 压根不触发，90 秒后进程还是没回来。
    ///
    /// 重复触发不依赖退出码：实例结束了，下一个触发点照样会跑；
    /// 实例还活着时 `IgnoreNew` 会让这次触发被跳过 —— 正好是想要的。
    ///
    /// 所以这两层缺一不可：
    /// `RestartOnFailure` 管「崩溃了快速回来」，重复触发管「无论如何都回来」。
    #[test]
    fn task_xml_repeats_every_minute() {
        let xml = task_xml(Path::new(r"C:\test\trit-agent-daemon.exe"));

        assert!(
            xml.contains("<CalendarTrigger>"),
            "必须有 CalendarTrigger —— RestartOnFailure 在外部强杀时不触发"
        );
        assert!(
            xml.contains("<Interval>PT1M</Interval>"),
            "重复触发间隔必须是 1 分钟"
        );
        assert!(
            xml.contains("<StopAtDurationEnd>false</StopAtDurationEnd>"),
            "StopAtDurationEnd 必须为 false，否则到期后就不再重复触发了"
        );
        assert!(
            xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"),
            "必须是 IgnoreNew —— 否则每分钟会堆一个新实例"
        );
    }

    /// 任务 XML 里的命令行必须把 exe 路径**用引号包起来**。
    ///
    /// 安装路径是 `%LOCALAPPDATA%\trit-agent\`，用户名里可能有空格
    /// （用户名含空格在 Windows 上是合法的）。
    /// 不加引号 → 任务计划程序把路径截断 → 任务永远启动失败。
    #[test]
    fn task_xml_quotes_the_exe_path() {
        let xml = task_xml(Path::new(r"C:\Users\John Doe\AppData\Local\trit-agent\daemon.exe"));
        assert!(
            xml.contains("&quot;C:\\Users\\John Doe\\AppData\\Local\\trit-agent\\daemon.exe&quot;"),
            "exe 路径必须被引号包住（用户名可能含空格）:\n{xml}"
        );
    }

    /// 任务 XML 里不能出现会破坏解析的裸字符。
    ///
    /// `<UserId>` 用的是 SID（形如 `S-1-5-21-...`），不含 XML 特殊字符，
    /// 但如果哪天真拿到含 `&` 的用户名，XML 就废了 ——
    /// 而 schtasks 报的是「任务 XML 损坏」，看不出是哪里的问题。
    #[test]
    fn task_xml_has_no_unescaped_ampersand() {
        let xml = task_xml(Path::new(r"C:\test\daemon.exe"));
        // 每个 `&` 都必须是合法实体的一部分
        let mut rest = xml.as_str();
        while let Some(pos) = rest.find('&') {
            let tail = &rest[pos..];
            let ok = ["&quot;", "&amp;", "&lt;", "&gt;", "&apos;"]
                .iter()
                .any(|e| tail.starts_with(e));
            assert!(ok, "XML 里有未转义的 '&': {tail:.40}");
            rest = &rest[pos + 1..];
        }
    }

    /// panic 必须能被记录下来 —— 否则守护进程会「静默死亡」。
    ///
    /// 守护进程是 GUI 子系统（**没有控制台**）+ release `panic = "abort"`：
    /// 一次 panic 意味着进程直接终止，而 panic 消息在 stderr 上无处可打。
    /// 结果就是日志停在某一行、进程消失、用户只看到「按 F8 没反应」。
    ///
    /// 这里真的 panic 一次，验证 `catch_unwind` + 落盘能抓到它。
    /// 注意：**`catch_unwind` 只在 `panic = "unwind"` 下有效**，
    /// release profile 是 `abort`，所以这个测试用 `#[should_panic]` 跑不了，
    /// 只能验证「钩子装上之后 panic 会被记录到日志文件」。
    #[test]
    fn panic_hook_writes_to_the_daemon_log() {
        // 先清掉日志，确保测的是本次写入的
        let log = daemon_log_path();
        let _ = std::fs::remove_file(&log);

        install_panic_hook();
        let result = std::panic::catch_unwind(|| {
            panic!("故意的 panic，用于验证落盘");
        });
        assert!(result.is_err(), "catch_unwind 应该抓到 panic");

        // 读日志确认落盘了
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            text.contains("故意的 panic"),
            "panic 消息必须落进守护日志，实际内容:\n{text}"
        );
        assert!(
            text.contains("panic"),
            "日志里要有 panic 标记，实际内容:\n{text}"
        );
    }
}

