//! Windows 原生输入注入（零依赖，直接 FFI user32）。
//!
//! 为什么不用 SendKeys / AutoHotkey：SendKeys 走的是消息队列，Minecraft 用的是
//! LWJGL 的原始键盘状态，收不到；必须走 `SendInput`（内核级注入到输入队列）。

use super::{Button, Input};
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

type Hwnd = isize;

const INPUT_MOUSE: u32 = 0;
const INPUT_KEYBOARD: u32 = 1;

const KEYEVENTF_KEYUP: u32 = 0x0002;
const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;

const SW_RESTORE: i32 = 9;

#[repr(C)]
#[derive(Clone, Copy)]
struct MouseInput {
    dx: i32,
    dy: i32,
    mouse_data: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KeybdInput {
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
union InputUnion {
    mi: MouseInput,
    ki: KeybdInput,
}

#[repr(C)]
struct RawInput {
    typ: u32,
    u: InputUnion,
}

#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

unsafe extern "system" {
    fn SendInput(n: u32, inputs: *const RawInput, size: i32) -> u32;
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn EnumWindows(cb: unsafe extern "system" fn(Hwnd, isize) -> i32, lparam: isize) -> i32;
    fn GetWindowTextW(h: Hwnd, buf: *mut u16, max: i32) -> i32;
    fn GetWindowTextLengthW(h: Hwnd) -> i32;
    fn IsWindowVisible(h: Hwnd) -> i32;
    fn SetForegroundWindow(h: Hwnd) -> i32;
    fn GetForegroundWindow() -> Hwnd;
    fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
    fn ClientToScreen(h: Hwnd, p: *mut Point) -> i32;
    fn GetClientRect(h: Hwnd, r: *mut Rect) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    fn ExitProcess(code: u32) -> !;
}

/// 全局按住状态：Ctrl+C 退出时用它把键全部松开，避免角色永远往前跑。
fn held() -> &'static Mutex<BTreeSet<u16>> {
    static HELD: OnceLock<Mutex<BTreeSet<u16>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(BTreeSet::new()))
}

unsafe extern "system" fn on_ctrl(_kind: u32) -> i32 {
    if let Ok(set) = held().lock() {
        for vk in set.iter() {
            send_key(*vk, false);
        }
    }
    send_mouse(0, 0, MOUSEEVENTF_LEFTUP | MOUSEEVENTF_RIGHTUP);
    unsafe { ExitProcess(0) }
}

/// 装上 Ctrl+C / 关窗 时的「松手保险」。
pub fn install_panic_handler() {
    unsafe {
        SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
}

fn send_key(vk: u16, down: bool) {
    let input = RawInput {
        typ: INPUT_KEYBOARD,
        u: InputUnion {
            ki: KeybdInput {
                vk,
                scan: 0,
                flags: if down { 0 } else { KEYEVENTF_KEYUP },
                time: 0,
                extra: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, std::mem::size_of::<RawInput>() as i32);
    }
}

fn send_mouse(dx: i32, dy: i32, flags: u32) {
    let input = RawInput {
        typ: INPUT_MOUSE,
        u: InputUnion {
            mi: MouseInput {
                dx,
                dy,
                mouse_data: 0,
                flags,
                time: 0,
                extra: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, std::mem::size_of::<RawInput>() as i32);
    }
}

unsafe extern "system" fn enum_cb(h: Hwnd, lparam: isize) -> i32 {
    let out = unsafe { &mut *(lparam as *mut Vec<(Hwnd, String)>) };
    if unsafe { IsWindowVisible(h) } == 0 {
        return 1;
    }
    let len = unsafe { GetWindowTextLengthW(h) };
    if len <= 0 {
        return 1;
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
    if n > 0 {
        out.push((h, String::from_utf16_lossy(&buf[..n as usize])));
    }
    1
}

/// 按标题关键字（不区分大小写）找窗口。
pub fn find_window(keyword: &str) -> Option<(Hwnd, String)> {
    let mut found: Vec<(Hwnd, String)> = Vec::new();
    unsafe {
        EnumWindows(enum_cb, &mut found as *mut _ as isize);
    }
    let kw = keyword.to_lowercase();
    found
        .into_iter()
        .find(|(_, t)| t.to_lowercase().contains(&kw))
}

/// 列出所有可见窗口标题，找不到游戏时用来排查。
pub fn list_windows() -> Vec<String> {
    let mut found: Vec<(Hwnd, String)> = Vec::new();
    unsafe {
        EnumWindows(enum_cb, &mut found as *mut _ as isize);
    }
    found.into_iter().map(|(_, t)| t).collect()
}

pub fn key_down(vk: u16) -> bool {
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

pub struct Win32Input {
    hwnd: Hwnd,
    title: String,
    pub sent: u64,
}

impl Win32Input {
    pub fn attach(keyword: &str) -> Option<Win32Input> {
        find_window(keyword).map(|(hwnd, title)| Win32Input {
            hwnd,
            title,
            sent: 0,
        })
    }

    pub fn hwnd(&self) -> Hwnd {
        self.hwnd
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    fn track(&self, vk: u16, down: bool) {
        if let Ok(mut set) = held().lock() {
            if down {
                set.insert(vk);
            } else {
                set.remove(&vk);
            }
        }
    }
}

impl Input for Win32Input {
    fn key(&mut self, vk: u16, down: bool) {
        self.track(vk, down);
        send_key(vk, down);
        self.sent += 1;
    }

    fn mouse_move(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        send_mouse(dx, dy, MOUSEEVENTF_MOVE);
        self.sent += 1;
    }

    fn button(&mut self, b: Button, down: bool) {
        let flags = match (b, down) {
            (Button::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (Button::Left, false) => MOUSEEVENTF_LEFTUP,
            (Button::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (Button::Right, false) => MOUSEEVENTF_RIGHTUP,
        };
        send_mouse(0, 0, flags);
        self.sent += 1;
    }

    fn release_all(&mut self) {
        let keys: Vec<u16> = held()
            .lock()
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        for vk in keys {
            send_key(vk, false);
        }
        if let Ok(mut set) = held().lock() {
            set.clear();
        }
        send_mouse(0, 0, MOUSEEVENTF_LEFTUP | MOUSEEVENTF_RIGHTUP);
    }

    fn focus(&mut self) -> bool {
        unsafe {
            ShowWindow(self.hwnd, SW_RESTORE);
            SetForegroundWindow(self.hwnd);
            GetForegroundWindow() == self.hwnd
        }
    }

    fn click_center(&mut self) {
        let mut rect = Rect::default();
        unsafe {
            if GetClientRect(self.hwnd, &mut rect) == 0 {
                return;
            }
        }
        let mut p = Point {
            x: (rect.right - rect.left) / 2,
            y: (rect.bottom - rect.top) / 2,
        };
        unsafe {
            ClientToScreen(self.hwnd, &mut p);
            SetCursorPos(p.x, p.y);
        }
        send_mouse(0, 0, MOUSEEVENTF_LEFTDOWN);
        send_mouse(0, 0, MOUSEEVENTF_LEFTUP);
    }
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// 把系统定时器精度提到 1ms。默认 15.6ms 会让 200Hz 循环变成 ~64Hz。
pub fn begin_high_res_timer() {
    unsafe {
        timeBeginPeriod(1);
    }
}

pub fn end_high_res_timer() {
    unsafe {
        timeEndPeriod(1);
    }
}

/// 按键边沿检测（按下那一下返回 true）。
pub struct Hotkeys {
    prev: Vec<(u16, bool)>,
}

impl Hotkeys {
    pub fn new(vks: &[u16]) -> Hotkeys {
        Hotkeys {
            prev: vks.iter().map(|v| (*v, false)).collect(),
        }
    }

    /// 返回本周期「刚被按下」的键。
    pub fn poll(&mut self) -> Vec<u16> {
        let mut out = Vec::new();
        for (vk, prev) in self.prev.iter_mut() {
            let now = key_down(*vk);
            if now && !*prev {
                out.push(*vk);
            }
            *prev = now;
        }
        out
    }
}
