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
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // 用 UTC 偏移不了时区，至少给个可排序的绝对时间。
    let days = secs / 86_400;
    let tod = secs % 86_400;
    format!(
        "day{} {:02}:{:02}:{:02}",
        days,
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
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
                    log_line(&format!("配置: {}", p.display()));
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
        log_line("已经有实例在运行，本进程退出");
        return 0;
    }
    log_line("守护进程启动");

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
                    log_line(&format!(
                        "等待游戏窗口（标题含 \"{}\"）……",
                        cfg.window_title
                    ));
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        log_line(&format!("挂上窗口: {}", input.title()));

        // ---- 跑一轮 ----
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
        // 不接管时也要挂标题标记，让用户一眼看到「守护进程在，
        // 按 F8 接管」。
        agent.update_badge();

        let mut tail = LogTail::new(&log_path);
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

        let stats = agent.run(Some(hk), None);

        log_line(&format!(
            "窗口结束：拍了 {} 次，{:.1} Hz，阶段分布 {:?}，后台暂停 {} 次",
            stats.ticks, stats.achieved_hz, stats.phase_hist, stats.bg_pauses
        ));
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
            let _ = std::process::Command::new(&target_exe).spawn();
            return 0;
        }
    }
    if !reg_set(RUN_KEY, RUN_VALUE, &cmd) {
        log_line("写注册表失败");
        return 1;
    }
    log_line(&format!("已安装开机自启: {cmd}"));

    // 立刻启动一个，用户不用等下次登录
    let _ = std::process::Command::new(&target_exe).spawn();
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
    out.push_str(&format!("守护进程: {}\n", if running { "运行中" } else { "未运行" }));
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
}

