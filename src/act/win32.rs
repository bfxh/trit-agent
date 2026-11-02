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
    fn IsWindow(h: Hwnd) -> i32;
    fn SetForegroundWindow(h: Hwnd) -> i32;
    fn GetForegroundWindow() -> Hwnd;
    fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
    fn ClientToScreen(h: Hwnd, p: *mut Point) -> i32;
    fn GetClientRect(h: Hwnd, r: *mut Rect) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    fn ExitProcess(code: u32) -> !;
    // ---- 命名对象：单实例互斥 + 停机信号 ----
    fn CreateMutexW(attrs: *mut core::ffi::c_void, initial_owner: i32, name: *const u16) -> isize;
    fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn CreateEventW(
        attrs: *mut core::ffi::c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> isize;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn SetEvent(h: isize) -> i32;
    fn WaitForSingleObject(h: isize, ms: u32) -> u32;
    fn CloseHandle(h: isize) -> i32;
    fn GetLastError() -> u32;
}

/// 命名对象句柄。0 = 无效。
pub type Handle = isize;

/// `GetLastError` 的「对象已存在」。
const ERROR_ALREADY_EXISTS: u32 = 183;
/// `WaitForSingleObject` 的「信号已置位」。
const WAIT_OBJECT_0: u32 = 0;
/// `WaitForSingleObject` 的「超时」。
const WAIT_TIMEOUT: u32 = 258;
/// `EVENT_MODIFY_STATE`
const EVENT_MODIFY_STATE: u32 = 0x0002;
/// `SYNCHRONIZE` —— 等待/查询对象状态所需的最小权限。
///
/// 互斥量**不能**用 `EVENT_MODIFY_STATE`（那是事件的权限位）：
/// 拿错权限会让 `OpenMutexW` 直接失败，于是「进程明明在跑，
/// 但状态查询说没跑」—— 排查时极具误导性。
const SYNCHRONIZE: u32 = 0x0010_0000;

/// 把 &str 编成 UTF-16（带结尾的 0）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 创建一个命名互斥量。返回 `(句柄, 是否此前已存在)`。
///
/// 「已存在」= 已经有另一个实例在跑。后台守护靠它保证全局只有一个，
/// 否则两个进程同时轮询 F8，**按一次会被切换两次 = 看起来毫无反应**。
pub fn create_named_mutex(name: &str) -> Option<(Handle, bool)> {
    let w = wide(name);
    let h = unsafe { CreateMutexW(std::ptr::null_mut(), 1, w.as_ptr()) };
    if h == 0 {
        return None;
    }
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    Some((h, existed))
}

/// 打开一个已存在的命名互斥量。`None` = 没有这个对象。
pub fn open_named_mutex(name: &str) -> Option<Handle> {
    let w = wide(name);
    let h = unsafe { OpenMutexW(SYNCHRONIZE, 0, w.as_ptr()) };
    if h == 0 {
        None
    } else {
        Some(h)
    }
}

/// 创建一个命名事件（**手动复位**）。
///
/// 手动复位是刻意的，不是随手写的：停机信号必须「粘住」。
/// `WaitForSingleObject` 碰到手动复位事件**不会**把它清掉，
/// 所以谁查都是「已置位」。
///
/// 用自动复位（Windows 的默认）会出事：信号会被**第一个等待者吃掉**。
/// 而停机信号有**两个**查询点 —— 主循环里查一次，外层循环再查一次。
/// 自动复位下第二次查就是「未置位」，于是守护进程收到停机信号后
/// 反而 exit 不掉，退回去继续等窗口 —— 表现是「stop 说成功了，进程还在」。
pub fn create_named_event(name: &str) -> Option<Handle> {
    let w = wide(name);
    // manual_reset = 1（手动复位），initial_state = 0（初始未置位）
    let h = unsafe { CreateEventW(std::ptr::null_mut(), 1, 0, w.as_ptr()) };
    if h == 0 {
        None
    } else {
        Some(h)
    }
}

/// 打开一个已存在的命名事件，用来给它置位。
pub fn open_named_event(name: &str) -> Option<Handle> {
    let w = wide(name);
    let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, w.as_ptr()) };
    if h == 0 {
        None
    } else {
        Some(h)
    }
}

/// 给事件置位（通知另一个进程）。
pub fn signal_event(h: Handle) -> bool {
    unsafe { SetEvent(h) != 0 }
}

/// 事件是否已经置位。
///
/// 对**手动复位**事件这是纯查询：`WaitForSingleObject` 不会清掉置位状态，
/// 所以查多少次都是同一个答案（这一点是停机信号能用的前提）。
pub fn event_is_signaled(h: Handle) -> bool {
    unsafe { WaitForSingleObject(h, 0) == WAIT_OBJECT_0 }
}

/// 关闭句柄。
pub fn close_handle(h: Handle) {
    if h != 0 {
        unsafe { CloseHandle(h) };
    }
}

/// 强制松开「我们可能按住」的键。
///
/// 用途：进程被强杀（任务管理器）时，注入的 key-down 会留在系统键盘
/// 状态里 —— 角色会一直往前走。新进程启动时先无条件松一遍，
/// 把上一次崩溃留下的状态清掉。
pub fn release_known_keys() {
    const KEYS: &[u16] = &[
        0x57, // W
        0x41, // A
        0x53, // S
        0x44, // D
        0x20, // 空格
        0x10, // Shift
        0xA0, // LShift
        0xA1, // RShift
    ];
    for &vk in KEYS {
        send_key(vk, false);
    }
    send_mouse(0, 0, MOUSEEVENTF_LEFTUP | MOUSEEVENTF_RIGHTUP);
}

/// 等待某个事件被置位，返回 true 表示收到了。
///
/// 超时返回 false —— 这样调用方还能顺便检查别的退出条件。
pub fn wait_event(h: Handle, ms: u32) -> bool {
    unsafe { WaitForSingleObject(h, ms) == WAIT_OBJECT_0 }
}

/// `WaitForSingleObject` 的超时常量。
pub const TIMEOUT: u32 = WAIT_TIMEOUT;

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
/// 这些窗口类**绝不能**当成游戏窗口。
///
/// 判据是类名而不是标题：浏览器的标题什么都有 —— 一个讲 Minecraft 模组
/// 的网页、一个叫「Minecraft 启动器」的应用、一个下载页，标题里全都
/// 有 "Minecraft"。但它们的**类名**是稳定的：Chromium 系列一律是
/// `Chrome_WidgetWin_*`。
///
/// 漏判的代价：AI 接管后所有按键（前进 / 攻击 / 空格）都打进浏览器，
/// 角色纹丝不动，而控制台日志一切正常 —— 排查起来极其困惑。
fn is_foreign_window_class(class_upper: &str) -> bool {
    const FOREIGN_PREFIXES: &[&str] = &[
        // Chromium 内核：Chrome / Edge / 各类 Electron 应用
        "CHROME_WIDGETWIN",
        // Firefox
        "MOZILLAWINDOWCLASS",
        // Windows 资源管理器 / 桌面 / 任务栏
        "CABINETWCLASS",
        "SHELL_TRAYWND",
        "PROGMAN",
        "WORKERW",
        // 控制台 / 各种宿主窗口
        "CONSOLEWINDOWCLASS",
        "MSCTFIMEUI",
    ];
    FOREIGN_PREFIXES
        .iter()
        .any(|p| class_upper.starts_with(p))
}

/// 给一个候选窗口打分，分高者中选，0 分表示不合格。
///
/// **类名 `GLFW*` 是决定性的**：Minecraft 走 LWJGL3，窗口类名固定是 `GLFW30`，
/// 而标题是整合包可以随便改的 —— 「烦人的村民」的窗口标题是
/// `Annoying Villagers - Annoying Villagers`，整个字符串里没有 "Minecraft"。
/// 先按标题过滤再找类名，等于在这种整合包上永远找不到窗口。
///
/// 标题命中关键字只加分、不减分：用来在同时开了两个客户端时挑对那一个。
///
/// 但**浏览器窗口必须直接排除**：Edge/Chrome 的标题里出现 "Minecraft"
/// 太常见了 —— 搜个「Minecraft 模组」就有好几个标签页。这窗口一旦被选中，
/// 后果是 AI 把 W / 空格 / 左键全打进浏览器，角色在游戏里一动不动，
/// 而控制台一切正常。
///
/// 判断依据用**窗口类名**（浏览器是 Chromium 的 `Chrome_WidgetWin_*`，
/// MC 是 LWJGL3 的 `GLFW*`），比标题可靠得多。
pub fn window_score(title: &str, class: &str, keyword: &str) -> i32 {
    let t = title.to_lowercase();
    let kw = keyword.to_lowercase();
    let c = class.to_uppercase();

    // 浏览器 / 启动器 / 资源管理器一票否决。
    // 这不是「优先级低」，是「绝不能往里发键」—— SendInput 是系统级的，
    // 打错窗口就是在别人的应用里乱敲键盘。
    if is_foreign_window_class(&c) {
        return 0;
    }

    let mut score = 0;
    if c.starts_with("GLFW") {
        score += 100;
    }
    if !kw.is_empty() && t.contains(&kw) {
        score += 10;
        if t.starts_with(&kw) {
            score += 2;
        }
    }
    score
}

/// 在候选窗口里挑游戏窗口。
///
/// 抽成纯函数是为了能测：这里错一次，AI 就是「跑起来了但一个键都没发出去」，
/// 而控制台看起来一切正常。
pub fn pick_window(cands: &[(String, String)], keyword: &str) -> Option<usize> {
    let mut best: Option<(usize, i32)> = None;
    for (i, (title, class)) in cands.iter().enumerate() {
        let s = window_score(title, class, keyword);
        if s > 0 && best.map(|(_, b)| s > b).unwrap_or(true) {
            best = Some((i, s));
        }
    }
    best.map(|(i, _)| i)
}

pub fn find_window(keyword: &str) -> Option<(Hwnd, String)> {
    let all = enum_windows();
    let pairs: Vec<(String, String)> = all
        .iter()
        .map(|w| (w.title.clone(), w.class.clone()))
        .collect();
    let i = pick_window(&pairs, keyword)?;
    let w = &all[i];
    Some((w.hwnd, w.title.clone()))
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

    fn is_alive(&mut self) -> bool {
        // 句柄还有效 = 游戏窗口还在。
        //
        // 句柄失效后 GetForegroundWindow 之类会拿一个野指针去比，
        // 而 SendInput 更会照打不误 —— 后台守护必须知道窗口没了，
        // 好赶紧松手重找，别对着空气按 W。
        unsafe { IsWindow(self.hwnd) != 0 }
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

    /// 候选窗口。真机上抓到的这一条是这次修复的起因：
    /// 整合包「烦人的村民」的窗口标题叫 `Annoying Villagers - ...`，
    /// 全串没有 "Minecraft"，而类名是 `GLFW30`。
    fn w(title: &str, class: &str) -> (String, String) {
        (title.to_string(), class.to_string())
    }

    #[test]
    fn glfw_class_wins_even_when_the_title_was_replaced() {
        let cands = vec![
            w("WorkBuddy", "Chrome_WidgetWin_1"),
            w("Axolotl Launcher", "Tauri Window"),
            w("Annoying Villagers - Annoying Villagers", "GLFW30"),
            w("烦人的村民 - 文件资源管理器", "CabinetWClass"),
        ];
        let i = pick_window(&cands, "minecraft").expect("类名 GLFW30 就该认出来");
        assert_eq!(
            cands[i].1, "GLFW30",
            "标题里没有 minecraft 不代表它不是游戏窗口"
        );
    }

    #[test]
    fn title_keyword_still_works_without_a_glfw_window() {
        let cands = vec![
            w("记事本", "Notepad"),
            w("Minecraft 1.18.2", "SunAwtFrame"),
        ];
        let i = pick_window(&cands, "minecraft").expect("标题命中也要能选中");
        assert_eq!(i, 1);
    }

    #[test]
    fn two_clients_prefer_the_one_matching_the_keyword() {
        // 同时开两个客户端：类名都是 GLFW30，靠标题分
        let cands = vec![
            w("Minecraft 1.18.2", "GLFW30"),
            w("另一个整合包", "GLFW30"),
        ];
        let i = pick_window(&cands, "minecraft").expect("应该挑中标题命中的那个");
        assert_eq!(i, 0);
    }

    #[test]
    fn no_candidate_means_none() {
        assert!(pick_window(&[], "minecraft").is_none());
        let cands = vec![w("记事本", "Notepad"), w("Chrome", "Chrome_WidgetWin_1")];
        assert!(pick_window(&cands, "minecraft").is_none());
    }

    /// 回归测试：浏览器窗口即使标题里有 "Minecraft" 也绝不能被选中。
    ///
    /// 真机上抓到的实例：Edge 打开了「Xaero 小地图更新——适用于…1.18.2…
    /// 的 Minecraft 模组」这个网页，标题里全是 minecraft。游戏没开的时候
    /// probe 把它当成了游戏窗口。
    ///
    /// 后果不是「窗口找错」这么轻：AI 接管后 SendInput 会把前进、攻击、
    /// 空格全打进浏览器，角色在游戏里一动不动，而日志一切正常。
    #[test]
    fn browser_window_with_minecraft_in_title_is_never_picked() {
        let cands = vec![
            w(
                "Xaero 小地图更新——适用于 1.18.2 的 Minecraft 模组！ - Microsoft Edge",
                "Chrome_WidgetWin_1",
            ),
            w("Minecraft 模组下载 - Chrome", "Chrome_WidgetWin_1"),
        ];
        assert!(
            pick_window(&cands, "minecraft").is_none(),
            "浏览器窗口绝不能被当成游戏窗口"
        );
        assert_eq!(window_score(&cands[0].0, &cands[0].1, "minecraft"), 0);
    }

    /// 浏览器窗口即使分数高也不能压过真正的游戏窗口。
    #[test]
    fn real_game_window_beats_a_browser_that_mentions_minecraft() {
        let cands = vec![
            w(
                "Xaero 小地图更新——适用于 1.18.2 的 Minecraft 模组！ - Edge",
                "Chrome_WidgetWin_1",
            ),
            w("Annoying Villagers - Annoying Villagers", "GLFW30"),
        ];
        let i = pick_window(&cands, "minecraft").expect("游戏窗口必须被选中");
        assert_eq!(cands[i].1, "GLFW30");
    }

    /// 反向锁：非浏览器的普通窗口该被选中时还得能选中。
    ///
    /// 「一票否决」不能连累正常候选 —— 老版本 MC 的窗口类名是 SunAwtFrame，
    /// 整合包启动器是 Tauri Window，这些都还得认。
    #[test]
    fn non_browser_candidates_are_not_affected_by_the_exclusion() {
        // 资源管理器：标题带 minecraft，但类名要排除
        assert_eq!(
            window_score("烦人的村民 - 文件资源管理器", "CabinetWClass", "minecraft"),
            0,
            "资源管理器不能当游戏窗口"
        );
        // 老版本 MC 的窗口类名是 SunAwtFrame：标题带关键字就得认
        assert_ne!(
            window_score("Minecraft 1.18.2", "SunAwtFrame", "minecraft"),
            0,
            "SunAwtFrame 不能被误排除"
        );
        // 整合包启动器：类名 Tauri Window，标题带 minecraft
        assert_ne!(
            window_score("Hello Minecraft! Launcher", "Tauri Window", "minecraft"),
            0,
            "启动器不该被浏览器规则误伤（标题命中仍算候选）"
        );
    }

    #[test]
    fn typing_charset_covers_every_command_we_send() {
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
