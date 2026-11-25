//! Win32 辅助函数：窗口显示/隐藏、前台窗口切换、写剪贴板、模拟粘贴。
//!
//! GPUI 在 Windows 上的 `App::hide()` 是空实现，所以窗口的显示/隐藏直接用原生 HWND 操作。

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::Foundation::{HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
    VK_SHIFT, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect, GetWindowTextW, HWND_TOPMOST, IsWindow,
    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos,
    SetWindowTextW, ShowWindow,
};
use windows::core::w;

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

pub fn window_title(raw: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd(raw), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// 显示窗口、切到前台并设置键盘焦点。返回之后键盘焦点是否在这个窗口上。
///
/// GPUI 自己处理 WM_ACTIVATE 时不会把键盘焦点交给窗口（它的 activate() 会显式 SetFocus），
/// 我们绕开了 activate()，所以必须自己调用 SetFocus，否则按键不会发到这个窗口。
pub fn show_and_focus(raw: isize) -> bool {
    unsafe {
        let _ = ShowWindow(hwnd(raw), SW_SHOW);
        let _ = SetForegroundWindow(hwnd(raw));
        let _ = SetFocus(Some(hwnd(raw)));
        GetFocus().0 as isize == raw
    }
}

/// 把焦点还给其他程序的窗口（只切前台，不改变它的显示状态）
pub fn focus(raw: isize) {
    unsafe {
        let _ = SetForegroundWindow(hwnd(raw));
    }
}

pub fn hide(raw: isize) {
    unsafe {
        let _ = ShowWindow(hwnd(raw), SW_HIDE);
    }
}

pub fn set_title(raw: isize, title: &str) {
    unsafe {
        let _ = SetWindowTextW(hwnd(raw), &windows::core::HSTRING::from(title));
    }
}

/// Windows 11 圆角（无边框弹窗默认是直角）
pub fn round_corners(raw: isize) {
    let pref = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd(raw),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

/// 窗口左上角的屏幕坐标
pub fn window_pos(raw: isize) -> Option<(i32, i32)> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd(raw), &mut rect) }.ok()?;
    Some((rect.left, rect.top))
}

pub fn move_to(raw: isize, (x, y): (i32, i32)) {
    unsafe {
        let _ = SetWindowPos(
            hwnd(raw),
            Some(HWND_TOPMOST),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 这个点（窗口左上角附近）是否还在某块显示器上（显示器可能被拔掉了）
pub fn is_on_screen((x, y): (i32, i32)) -> bool {
    let probe = POINT {
        x: x + 40,
        y: y + 20,
    };
    !unsafe { MonitorFromPoint(probe, MONITOR_DEFAULTTONULL) }.is_invalid()
}

/// 把窗口放到鼠标所在显示器工作区的水平居中、垂直偏上（约 1/4 处）
pub fn center_on_cursor_monitor(raw: isize) {
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() {
            return;
        }
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd(raw), &mut rect).is_err() {
            return;
        }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let work = info.rcWork;
        let x = work.left + (work.right - work.left - w) / 2;
        let y = work.top + ((work.bottom - work.top - h) as f32 * 0.28) as i32;
        let _ = SetWindowPos(
            hwnd(raw),
            Some(HWND_TOPMOST),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[derive(Debug)]
pub struct PasteReport {
    /// 目标窗口成为前台所花的时间
    pub focus_ms: f64,
    /// 是否成功切回目标窗口
    pub focused: bool,
    pub target_title: String,
}

/// 切回之前的前台窗口并模拟 Ctrl+V。
pub fn restore_and_paste(target: isize) -> PasteReport {
    let t0 = Instant::now();
    let title = window_title(target);
    let valid = unsafe { IsWindow(Some(hwnd(target))) }.as_bool();
    if !valid {
        return PasteReport {
            focus_ms: 0.0,
            focused: false,
            target_title: title,
        };
    }

    unsafe {
        let _ = SetForegroundWindow(hwnd(target));
    }
    let deadline = t0 + Duration::from_millis(300);
    let mut focused = false;
    while Instant::now() < deadline {
        if foreground() == target {
            focused = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let focus_ms = t0.elapsed().as_secs_f64() * 1000.0;

    if focused {
        send_ctrl_v();
    }
    PasteReport {
        focus_ms,
        focused,
        target_title: title,
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 粘贴片段后把光标移回 `{cursor}` 的位置：按 `n` 次 ←
pub fn send_left(n: usize) {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_LEFT;
    let inputs: Vec<INPUT> = (0..n.min(2000))
        .flat_map(|_| [key(VK_LEFT, false), key(VK_LEFT, true)])
        .collect();
    if !inputs.is_empty() {
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// 本地时间（片段里的 `{date}` `{time}`）
pub fn local_time() -> tessel_core::template::LocalTime {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    tessel_core::template::LocalTime {
        year: t.wYear,
        month: t.wMonth as u8,
        day: t.wDay as u8,
        hour: t.wHour as u8,
        minute: t.wMinute as u8,
        weekday: t.wDayOfWeek as u8,
    }
}

/// 热键触发时按住的修饰键还没松开（这时发 Ctrl+V 会变成 Ctrl+Alt+Shift+V 之类）
pub fn modifiers_held() -> bool {
    [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0)
}

/// 发送一个没有含义的按键（0xE8 未分配）。在按住 Alt / Win / Shift 的热键之后发一下，
/// 松开修饰键时系统就不会当成"单独按了 Alt"（激活菜单栏）、"单独按了 Win"（打开开始菜单），
/// 微软拼音也不会当成"单独按了 Shift"切换中英文。
pub fn send_mask_key() {
    let mask = VIRTUAL_KEY(0xE8);
    let inputs = [key(mask, false), key(mask, true)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

pub fn send_ctrl_v() {
    let inputs = [
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

// ───────────────────────────── 复制提示浮层 ─────────────────────────────

/// 浮层窗口：不抢焦点、不在任务栏和 Alt+Tab 里出现、没有系统画的圆角边框
/// （窗口本身透明，胶囊和阴影都是自己画的）
pub fn make_overlay(raw: isize) {
    use windows::Win32::Graphics::Dwm::DWMWCP_DONOTROUND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST,
    };
    unsafe {
        let style = GetWindowLongPtrW(hwnd(raw), GWL_EXSTYLE);
        let extra = (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST).0 as isize;
        SetWindowLongPtrW(hwnd(raw), GWL_EXSTYLE, style | extra);
        // GPUI 的弹出窗口是"有边框的普通窗口"样式，Windows 11 会沿整个窗口矩形画阴影和边框，
        // 透明区域外面就多出一圈方框。改成无边框弹出样式，并关掉系统的非客户区绘制。
        use windows::Win32::Graphics::Dwm::{DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY};
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_STYLE, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOZORDER, WS_POPUP,
        };
        SetWindowLongPtrW(hwnd(raw), GWL_STYLE, WS_POPUP.0 as isize);
        let policy = DWMNCRP_DISABLED;
        let _ = DwmSetWindowAttribute(
            hwnd(raw),
            DWMWA_NCRENDERING_POLICY,
            &policy as *const _ as *const _,
            std::mem::size_of_val(&policy) as u32,
        );
        let _ = SetWindowPos(
            hwnd(raw),
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd(raw),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
        // Windows 11 给窗口画的 1px 边框也去掉（DWMWA_COLOR_NONE）
        let none: u32 = 0xFFFF_FFFE;
        let _ = DwmSetWindowAttribute(
            hwnd(raw),
            windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR,
            &none as *const _ as *const _,
            std::mem::size_of_val(&none) as u32,
        );
    }
}

/// 把窗口摆到 `over` 窗口的正中（没有时放到鼠标所在屏幕的中间），不超出屏幕工作区
pub fn place_over(raw: isize, over: Option<isize>) {
    unsafe {
        let mut own = RECT::default();
        if GetWindowRect(hwnd(raw), &mut own).is_err() {
            return;
        }
        let (w, h) = (own.right - own.left, own.bottom - own.top);
        let center = match over {
            Some(o) => {
                let mut r = RECT::default();
                if GetWindowRect(hwnd(o), &mut r).is_err() {
                    return;
                }
                POINT {
                    x: (r.left + r.right) / 2,
                    y: (r.top + r.bottom) / 2,
                }
            }
            None => {
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                    return;
                }
                let work = info.rcWork;
                POINT {
                    x: (work.left + work.right) / 2,
                    y: (work.top + work.bottom) / 2,
                }
            }
        };
        let monitor = MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let work = info.rcWork;
        let x = (center.x - w / 2).clamp(work.left, (work.right - w).max(work.left));
        let y = (center.y - h / 2).clamp(work.top, (work.bottom - h).max(work.top));
        use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
        let _ = SetWindowPos(
            hwnd(raw),
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 不激活地显示在最上层
pub fn show_no_activate(raw: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOMOVE, SWP_SHOWWINDOW};
    unsafe {
        let _ = SetWindowPos(
            hwnd(raw),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

/// 放到当前前台窗口所在显示器的底部中间（任务栏上方）
pub fn place_bottom_center(raw: isize) {
    use windows::Win32::Graphics::Gdi::MonitorFromWindow;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    unsafe {
        let fg = GetForegroundWindow();
        let monitor = if fg.is_invalid() {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST)
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd(raw), &mut rect).is_err() {
            return;
        }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let scale = GetDpiForWindow(hwnd(raw)).max(96) as f32 / 96.0;
        let work = info.rcWork;
        let x = work.left + (work.right - work.left - w) / 2;
        let y = work.bottom - h - (40.0 * scale) as i32;
        let _ = SetWindowPos(
            hwnd(raw),
            Some(HWND_TOPMOST),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 当前进程（不含 WebView 之类的子进程，GPUI 也没有子进程）的内存占用，单位 MB。
pub fn memory_mb() -> (f64, f64) {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        let _ = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        );
    }
    (
        counters.WorkingSetSize as f64 / 1_048_576.0,
        counters.PrivateUsage as f64 / 1_048_576.0,
    )
}

/// 跨进程信号（命名的自动重置事件）：另一个实例发来的请求
pub struct InstanceSignal(isize);

impl InstanceSignal {
    /// 阻塞等待下一次请求；句柄失效时返回 false
    pub fn wait(&self) -> bool {
        unsafe { WaitForSingleObject(HANDLE(self.0 as *mut _), INFINITE).0 == 0 }
    }
}

// 句柄只用于等待，可以在线程间移动
unsafe impl Send for InstanceSignal {}

pub struct InstanceSignals {
    /// "显示弹窗"（双击快捷方式、再次启动）
    pub show: InstanceSignal,
    /// "打开设置"（以 --settings 启动）
    pub settings: InstanceSignal,
}

/// 单实例：第一个实例返回 `Some`，用来接收之后的"请显示"请求；
/// 已经有实例在运行时，通知它显示出来并返回 `None`（调用方应直接退出）。
/// 单实例：第一个实例返回 `Some`，用来接收之后的请求；
/// 已经有实例在运行时，把请求（显示弹窗 / 打开设置）转给它并返回 `None`（调用方应直接退出）。
///
/// `instance_key`：区分不同数据目录的实例（开发测试时可以和正式实例同时运行），默认为空。
pub fn acquire_single_instance(instance_key: &str, want_settings: bool) -> Option<InstanceSignals> {
    let event = |kind: &str| {
        let name = windows::core::HSTRING::from(format!(r"Local\Tessel.{kind}{instance_key}"));
        unsafe { CreateEventW(None, false, false, &name) }.ok()
    };
    let show = event("ShowRequest")?;
    // 必须紧跟在第一次 CreateEventW 之后读取
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let settings = event("SettingsRequest")?;
    if already_running {
        unsafe {
            // 我们是用户刚启动的进程，拥有前台权限；把它让给已经在运行的那个实例
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = SetEvent(if want_settings { settings } else { show });
        }
        return None;
    }
    Some(InstanceSignals {
        show: InstanceSignal(show.0 as isize),
        settings: InstanceSignal(settings.0 as isize),
    })
}

/// 普通窗口（设置窗口）切到前台：最小化了就先还原
pub fn restore_and_focus(raw: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE};
    unsafe {
        let cmd = if IsIconic(hwnd(raw)).as_bool() {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        let _ = ShowWindow(hwnd(raw), cmd);
        let _ = SetForegroundWindow(hwnd(raw));
    }
}

// ───────────────────────────── 开机自启 ─────────────────────────────

const RUN_KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: windows::core::PCWSTR = w!("Tessel");

fn autostart_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --background", exe.display()))
}

/// 注册表里登记的自启命令是否指向当前这个 exe
pub fn autostart_enabled() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if err.is_err() {
        return false;
    }
    let len = (size as usize / 2).saturating_sub(1);
    let value = String::from_utf16_lossy(&buf[..len]);
    autostart_command().is_some_and(|cmd| cmd.eq_ignore_ascii_case(&value))
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    let err = if enabled {
        let cmd = autostart_command().ok_or("无法获取程序路径")?;
        let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(wide.as_ptr() as *const _),
                (wide.len() * 2) as u32,
            )
        }
    } else {
        let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) };
        // 本来就没有登记，也算成功
        if err == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        err
    };
    err.ok().map_err(|e| e.to_string())
}

/// Windows 显示语言是不是中文（简体、繁体都算）
pub fn system_is_chinese() -> bool {
    const LANG_CHINESE: u16 = 0x04;
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    lang & 0x3FF == LANG_CHINESE
}

/// 系统确认框，返回用户是否点了"确定"
pub fn confirm(title: &str, text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IDOK, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
    };
    let result = unsafe {
        MessageBoxW(
            None,
            &windows::core::HSTRING::from(text),
            &windows::core::HSTRING::from(title),
            MB_OKCANCEL | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    result == IDOK
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 会读写当前用户的 Run 注册表项，结束时恢复原状。
    #[test]
    fn autostart_roundtrip() {
        let original = autostart_enabled();
        set_autostart(true).unwrap();
        assert!(autostart_enabled());
        set_autostart(false).unwrap();
        assert!(!autostart_enabled());
        // 没有登记时再关闭也应成功
        set_autostart(false).unwrap();
        set_autostart(original).unwrap();
        assert_eq!(autostart_enabled(), original);
    }
}
