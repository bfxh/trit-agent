//! 输出层：只暴露「按键 / 移动鼠标 / 点击」三种原子动作。
//! 上层（状态机）完全不知道自己在操作什么游戏。

pub mod dry;
#[cfg(windows)]
pub mod win32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Right,
}

pub trait Input {
    fn key(&mut self, vk: u16, down: bool);
    fn mouse_move(&mut self, dx: i32, dy: i32);
    fn button(&mut self, b: Button, down: bool);
    fn release_all(&mut self);
    fn focus(&mut self) -> bool;
    /// 把鼠标移到窗口客户区中心（点重生按钮用）
    fn click_center(&mut self);

    /// 目标窗口是否在前台。
    ///
    /// 这是必须品，不是可选项：`SendInput` 注入的是**系统级**输入，
    /// 目标窗口失焦时它依然会打到当前前台窗口上。人一按 Alt+Tab 去看视频，
    /// 角色就在浏览器里一路 `wwwwww` + 空格 + 左键连点。
    /// 上层拿到 false 时一律不注入。
    fn is_foreground(&mut self) -> bool {
        true
    }

    /// 在窗口标题前挂一个状态标记，让「AI 到底开没开」一眼可见。
    /// `None` = 摘掉标记。默认实现什么也不做（无窗口的仿真实现）。
    fn badge(&mut self, _text: Option<&str>) {}

    /// 周期性重设标记 —— 游戏自己也会改标题（切世界、进服务器），
    /// 不刷新就会被覆盖掉。默认什么也不做。
    fn refresh_badge(&mut self) {}

    /// 测试钩子：把目标窗口标记成前台/后台，用来验证失焦门控。
    /// 生产实现（win32）不覆盖 —— 它的前台状态是从系统真读的。
    fn set_foreground(&mut self, _on: bool) {}

    /// 测试钩子：读回最近挂上的标题标记。
    fn last_badge(&self) -> Option<String> {
        None
    }
}

/// 虚拟键码。只列出这里用得到的。
pub mod vk {
    pub const W: u16 = 0x57;
    pub const A: u16 = 0x41;
    pub const S: u16 = 0x53;
    pub const D: u16 = 0x44;
    pub const SPACE: u16 = 0x20;
    pub const SHIFT: u16 = 0x10;
    pub const CTRL: u16 = 0x11;
    pub const E: u16 = 0x45;
    pub const F5: u16 = 0x74;
    pub const DIGIT_1: u16 = 0x31;
}
