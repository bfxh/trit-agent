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
    fn SetWindowTextW(h: Hwnd, text: *const u16) -> i32;
    fn GetClassNameW(h: Hwnd, buf: *mut u16, max: i32) -> i32;
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

/// 枚举到的一个窗口。
pub struct WindowInfo {
    pub hwnd: Hwnd,
    pub title: String,
    /// 窗口类名。Minecraft 走 LWJGL3，类名是 `GLFW30` —— 这比标题可靠得多。
    pub class: String,
}

unsafe extern "system" fn enum_cb(h: Hwnd, lparam: isize) -> i32 {
    let out = unsafe { &mut *(lparam as *mut Vec<WindowInfo>) };
    if unsafe { IsWindowVisible(h) } == 0 {
        return 1;
    }
    let len = unsafe { GetWindowTextLengthW(h) };
    if len <= 0 {
        return 1;
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return 1;
    }
    let mut cbuf = [0u16; 64];
    let cn = unsafe { GetClassNameW(h, cbuf.as_mut_ptr(), cbuf.len() as i32) };
    let class = if cn > 0 {
        String::from_utf16_lossy(&cbuf[..cn as usize])
    } else {
        String::new()
    };
    out.push(WindowInfo {
        hwnd: h,
        title: String::from_utf16_lossy(&buf[..n as usize]),
        class,
    });
    1
}

fn enum_windows() -> Vec<WindowInfo> {
    let mut found: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(enum_cb, &mut found as *mut _ as isize);
    }
    found
}

/// 按标题关键字（不区分大小写）找窗口，**优先认窗口类名**。
///
/// 只按「标题包含关键字」找会抓错：HMCL 启动器的窗口标题是
/// `Hello Minecraft! Launcher`，同样包含 minecraft。抓错了的后果是
/// 给启动器改标题、对着启动器发键，而游戏那边一动不动。
///
/// 所以顺序是：标题命中且类名是 GLFW（真的游戏窗口）→ 标题以关键字开头
/// → 标题包含关键字。
pub fn find_window(keyword: &str) -> Option<(Hwnd, String)> {
    let kw = keyword.to_lowercase();
    let hits: Vec<WindowInfo> = enum_windows()
        .into_iter()
        .filter(|w| w.title.to_lowercase().contains(&kw))
        .collect();
    let pick = hits
        .iter()
        .find(|w| w.class.to_uppercase().starts_with("GLFW"))
        .or_else(|| hits.iter().find(|w| w.title.to_lowercase().starts_with(&kw)))
        .or_else(|| hits.first());
    pick.map(|w| (w.hwnd, w.title.clone()))
}

/// 列出所有可见窗口，找不到游戏时用来排查。
/// 顺带打印类名 —— 光看标题根本分不清启动器和游戏。
pub fn list_windows() -> Vec<String> {
    enum_windows()
        .into_iter()
        .map(|w| format!("{} [{}]", w.title, w.class))
        .collect()
}

pub fn key_down(vk: u16) -> bool {
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

fn window_title_of(h: Hwnd) -> String {
    let len = unsafe { GetWindowTextLengthW(h) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

fn set_window_title(h: Hwnd, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SetWindowTextW(h, wide.as_ptr());
    }
}

/// 我们自己往标题上挂的标记。剥的时候只认这几个，免得把游戏自己的方括号吃了。
const BADGES: [&str; 3] = ["[AI 接管]", "[AI 待机]", "[已交还]"];

fn strip_badge(title: &str) -> String {
    for b in BADGES {
        if let Some(rest) = title.strip_prefix(b) {
            return rest.trim_start().to_string();
        }
    }
    title.to_string()
}

pub struct Win32Input {
    hwnd: Hwnd,
    title: String,
    /// 当前想挂在标题前的标记
    badge_text: Option<String>,
    pub sent: u64,
}

impl Win32Input {
    pub fn attach(keyword: &str) -> Option<Win32Input> {
        find_window(keyword).map(|(hwnd, title)| Win32Input {
            hwnd,
            title,
            badge_text: None,
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

    fn is_foreground(&mut self) -> bool {
        // 严格按句柄判等，**不做标题兜底**。
        //
        // 一开始写了「前台窗口标题含 minecraft 也算」，结果反了：浏览器里
        // 标题是 "minecraft - Google 搜索" 时会被当成游戏在前台，AI 就往
        // 浏览器里敲 w、点左键。误判成「前台」的代价远大于误判成「后台」
        // （后者只是 AI 不动手，标题上写着 [AI 待机] 一眼能看出来）。
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    fn badge(&mut self, text: Option<&str>) {
        self.badge_text = text.map(|s| s.to_string());
        self.refresh_badge();
    }

    fn refresh_badge(&mut self) {
        // 游戏切世界/进服务器时会自己改标题，所以每次都重新读一遍再拼，
        // 不然标记要么被覆盖，要么把旧世界名一路带到新世界。
        let cur = window_title_of(self.hwnd);
        if cur.is_empty() {
            return;
        }
        let base = strip_badge(&cur);
        let want = match &self.badge_text {
            Some(t) => format!("{t} {base}"),
            None => base,
        };
        if cur != want {
            set_window_title(self.hwnd, &want);
        }
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
