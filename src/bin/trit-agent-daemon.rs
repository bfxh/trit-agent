//! 后台守护进程入口（无控制台窗口）。
//!
//! 用 Windows GUI 子系统编译：双击/开机自启都**不会弹黑窗口**。
//! 所有反馈写进 `trit-agent-daemon.log`（exe 同目录）。
//!
//! 用法：
//!   trit-agent-daemon.exe            正常跑（守护，等 F8）
//!   trit-agent-daemon.exe --install  安装开机自启
//!   trit-agent-daemon.exe --uninstall 卸载开机自启
//!
//! 前台模式（`trit-agent.exe run`）和它共用一个全局互斥量，
//! 保证同一时刻只有一个进程在监听 F8。

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--install") {
        std::process::exit(trit_agent::daemon::install_autostart());
    }
    if args.iter().any(|a| a == "--uninstall") {
        std::process::exit(trit_agent::daemon::uninstall_autostart());
    }
    if args.iter().any(|a| a == "--status") {
        std::process::exit(trit_agent::daemon::print_status());
    }

    std::process::exit(trit_agent::daemon::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("后台守护进程只在 Windows 上可用。");
    std::process::exit(2);
}
