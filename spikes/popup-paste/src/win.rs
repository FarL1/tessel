//! Win32 辅助函数：窗口显示/隐藏、前台窗口切换、写剪贴板、模拟粘贴。
//!
//! GPUI 在 Windows 上的 `App::hide()` 是空实现，所以窗口的显示/隐藏直接用原生 HWND 操作。

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, IsWindow, SW_HIDE, SW_SHOW, SetForegroundWindow,
    ShowWindow,
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

pub fn show_and_focus(raw: isize) {
    unsafe {
        let _ = ShowWindow(hwnd(raw), SW_SHOW);
        let _ = SetForegroundWindow(hwnd(raw));
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
