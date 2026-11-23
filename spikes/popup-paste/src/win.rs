//! Win32 辅助函数：窗口显示/隐藏、前台窗口切换、写剪贴板、模拟粘贴。
//!
//! GPUI 在 Windows 上的 `App::hide()` 是空实现，所以窗口的显示/隐藏直接用原生 HWND 操作。

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    SendInput, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect, GetWindowTextW, HWND_TOPMOST, IsWindow,
    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos, ShowWindow,
};

const CF_UNICODETEXT: u32 = 13;

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

pub fn set_clipboard_text(owner: isize, text: &str) -> windows::core::Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        OpenClipboard(Some(hwnd(owner)))?;
        let result = (|| {
            EmptyClipboard()?;
            let hmem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
            let ptr = GlobalLock(hmem) as *mut u16;
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(hmem);
            // 成功后内存归系统所有，不能再释放
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hmem.0)))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
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

fn send_ctrl_v() {
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
