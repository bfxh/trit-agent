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
