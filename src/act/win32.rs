//! Windows 原生输入注入（零依赖，直接 FFI user32）。
//!
//! 为什么不用 SendKeys / AutoHotkey：SendKeys 走的是消息队列，Minecraft 用的是
//! LWJGL 的原始键盘状态，收不到；必须走 `SendInput`（内核级注入到输入队列）。

use super::{Button, Input};
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

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

    fn type_command(&mut self, cmd: &str) -> bool {
        // 不在前台就绝不能打：字符会落到浏览器地址栏、IDE、搜索框里
        if !self.is_foreground() {
            return false;
        }
        let chars: Vec<char> = cmd.chars().collect();
        let Some(&head) = chars.first() else {
            return false;
        };

        // 斜杠用「按 `/` 键」表达（MC 里它会直接开命令框并预填 `/`），
        // 不作为普通字符打，否则会开出两次输入框。
        let body: &[char] = if head == '/' { &chars[1..] } else { &chars[..] };

        // **先校验再动手**：只要有一个字符打不出来，整条放弃。
        // 否则会发出半条指令 + 一个回车，游戏里就是一条不存在的命令 ——
        // 红字刷屏，而且从现象完全看不出是「有个字符没映射」。
        if body.iter().any(|c| vk_for_char(*c).is_none()) {
            return false;
        }

        // 打字前先把自己按住的键全松开，否则 WASD 会插进字符中间
        self.release_all();
        std::thread::sleep(Duration::from_millis(60));

        if head == '/' {
            tap_key(VK_OEM_2, false);
            std::thread::sleep(Duration::from_millis(80));
        } else {
            // 普通聊天内容：先按 T 开聊天框
            tap_key(VK_T, false);
            std::thread::sleep(Duration::from_millis(80));
        }

        for &c in body {
            if let Some((vk, shift)) = vk_for_char(c) {
                tap_key(vk, shift);
                self.sent += 1;
            }
        }
        tap_key(VK_RETURN, false);
        self.sent += 1;
        true
    }
}

const VK_RETURN: u16 = 0x0D;
const VK_T: u16 = 0x54;
const VK_OEM_2: u16 = 0xBF; // `/` `?`
const VK_OEM_1: u16 = 0xBA; // `;` `:`
const VK_OEM_MINUS: u16 = 0xBD; // `-` `_`
const VK_OEM_PLUS: u16 = 0xBB; // `=` `+`
const VK_OEM_COMMA: u16 = 0xBC;
const VK_OEM_PERIOD: u16 = 0xBE;
const VK_OEM_4: u16 = 0xDB; // `[`
const VK_OEM_6: u16 = 0xDD; // `]`
const VK_OEM_7: u16 = 0xDE; // `'` `"`
const VK_SHIFT_L: u16 = 0xA0;

/// 字符 → (虚拟键码, 要不要按 Shift)。**按 US 布局**。
///
/// 只覆盖发指令用得到的字符集。中文、全角符号一律返回 None —— 与其猜，
/// 不如让调用方知道「这条指令发不全」。
fn vk_for_char(c: char) -> Option<(u16, bool)> {
    let v = c as u32;
    Some(match c {
        'a'..='z' => (0x41 + (v - 'a' as u32) as u16, false),
        'A'..='Z' => (0x41 + (v - 'A' as u32) as u16, true),
        '0'..='9' => (0x30 + (v - '0' as u32) as u16, false),
        ' ' => (0x20, false),
        '/' => (VK_OEM_2, false),
        '?' => (VK_OEM_2, true),
        ';' => (VK_OEM_1, false),
        ':' => (VK_OEM_1, true),
        '-' => (VK_OEM_MINUS, false),
        '_' => (VK_OEM_MINUS, true),
        '=' => (VK_OEM_PLUS, false),
        '+' => (VK_OEM_PLUS, true),
        ',' => (VK_OEM_COMMA, false),
        '<' => (VK_OEM_COMMA, true),
        '.' => (VK_OEM_PERIOD, false),
        '>' => (VK_OEM_PERIOD, true),
        '[' => (VK_OEM_4, false),
        '{' => (VK_OEM_4, true),
        ']' => (VK_OEM_6, false),
        '}' => (VK_OEM_6, true),
        '\'' => (VK_OEM_7, false),
        '"' => (VK_OEM_7, true),
        '@' => (0x32, true), // Shift+2
        '!' => (0x31, true), // Shift+1
        '*' => (0x38, true), // Shift+8
        _ => return None,
    })
}

/// 打一个字符：按下、停 4ms、松开。Shift 包在外层。
///
/// 4ms 是按帧采样的下限试出来的：MC 每帧约 16ms，down/up 都短于 1ms 的脉冲
/// 在 GLFW 的字符回调里会被吞（和攻击键踩的是同一个坑）。
fn tap_key(vk: u16, shift: bool) {
    if shift {
        send_key(VK_SHIFT_L, true);
        std::thread::sleep(Duration::from_millis(2));
    }
    send_key(vk, true);
    std::thread::sleep(Duration::from_millis(4));
    send_key(vk, false);
    if shift {
        std::thread::sleep(Duration::from_millis(2));
        send_key(VK_SHIFT_L, false);
    }
    std::thread::sleep(Duration::from_millis(4));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_charset_covers_every_command_we_send() {
        // `squad` 的四个指令 + 可能的 /execute 参数形式
        let sample = "/function squad:engage";
        for c in sample.chars().filter(|c| *c != '/') {
            assert!(vk_for_char(c).is_some(), "字符 {c:?} 打不出来，指令会发不全");
        }
        for c in "_-:.[]=@!\"".chars() {
            assert!(vk_for_char(c).is_some(), "符号 {c:?} 没映射");
        }
    }

    #[test]
    fn non_ascii_is_rejected_instead_of_guessed() {
        // 与其猜中文怎么打，不如明确说打不出来 —— 调用方会整条放弃
        assert!(vk_for_char('中').is_none());
        assert!(vk_for_char('（').is_none());
        assert!(vk_for_char('，').is_none());
    }

    #[test]
    fn shift_is_flagged_for_shifted_symbols() {
        assert_eq!(vk_for_char(':'), Some((VK_OEM_1, true)));
        assert_eq!(vk_for_char('_'), Some((VK_OEM_MINUS, true)));
        // 未上档的不该多按 Shift
        assert_eq!(vk_for_char('/'), Some((VK_OEM_2, false)));
        assert_eq!(vk_for_char('a'), Some((0x41, false)));
        assert_eq!(vk_for_char('A'), Some((0x41, true)));
    }

    #[test]
    fn badge_prefix_is_stripped_before_recomposing() {
        // 不剥干净的话，每 500ms 刷新一次就会把前缀叠上去：
        // [AI 接管] [AI 接管] [AI 接管] Minecraft
        assert_eq!(strip_badge("[AI 接管] Minecraft 1.18.2"), "Minecraft 1.18.2");
        assert_eq!(strip_badge("[AI 待机] 新的世界 - Minecraft"), "新的世界 - Minecraft");
        assert_eq!(strip_badge("Minecraft 1.18.2"), "Minecraft 1.18.2");
    }
}
