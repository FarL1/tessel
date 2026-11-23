//! Windows 实现：消息窗口 + `AddClipboardFormatListener` 事件驱动监听。
//!
//! 关键点（来自 M0 验证，见 docs/m0-report.md #4）：
//! - 很多程序写剪贴板分两步，每次复制会触发两次通知 → 40ms 防抖合并
//! - 隐私标记命中时不读取内容
//! - 只读取需要的格式（GetClipboardData 会触发源程序的延迟渲染，开销可能很大）

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tessel_core::{NewClip, Payload};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, EnumClipboardFormats,
    GetClipboardData, GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, HWND_MESSAGE, KillTimer, MSG, RegisterClassW, SetTimer,
    WINDOW_EX_STYLE, WM_CLIPBOARDUPDATE, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use crate::WriteContent;
use crate::image_conv::{dib_to_png, png_dimensions, png_to_dib};

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;

const DEBOUNCE_MS: u32 = 40;
const DEBOUNCE_TIMER: usize = 1;
/// 超过这个大小的图片不记录
const MAX_IMAGE_BYTES: usize = 40 * 1024 * 1024;

fn reg(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) }
}

struct Formats {
    png: u32,
    exclude_monitor: u32,
    can_include_history: u32,
    viewer_ignore: u32,
}

impl Formats {
    fn new() -> Self {
        Self {
            png: reg("PNG"),
            exclude_monitor: reg("ExcludeClipboardContentFromMonitorProcessing"),
            can_include_history: reg("CanIncludeInClipboardHistory"),
            viewer_ignore: reg("Clipboard Viewer Ignore"),
        }
    }
}

type Callback = Box<dyn Fn(NewClip)>;

struct WatcherState {
    formats: Formats,
    paused: Arc<AtomicBool>,
    callback: Callback,
    /// exe 路径 → 显示名 的缓存
    app_names: HashMap<String, Option<String>>,
}

thread_local! {
    static STATE: RefCell<Option<WatcherState>> = const { RefCell::new(None) };
}

pub fn spawn_watcher(
    paused: Arc<AtomicBool>,
    on_capture: impl Fn(NewClip) + Send + 'static,
) -> std::io::Result<()> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("tessel-clipboard-watcher".into())
        .spawn(move || {
            STATE.with(|s| {
                *s.borrow_mut() = Some(WatcherState {
                    formats: Formats::new(),
                    paused,
                    callback: Box::new(on_capture),
                    app_names: HashMap::new(),
                })
            });
            match create_listener_window() {
                Ok(_) => {
                    let _ = ready_tx.send(Ok(()));
                    unsafe {
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                            DispatchMessageW(&msg);
                        }
                    }
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(std::io::Error::other(e.to_string())));
                }
            }
        })?;
    ready_rx
        .recv()
        .unwrap_or_else(|_| Err(std::io::Error::other("watcher thread exited")))
}

fn create_listener_window() -> windows::core::Result<HWND> {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("TesselClipboardListener");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);
        // HWND_MESSAGE：只收消息的隐藏窗口
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("Tessel clipboard listener"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinstance),
            None,
        )?;
        AddClipboardFormatListener(hwnd)?;
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            // 重复 SetTimer 会重置计时：连续通知只在最后一次之后处理一次
            unsafe { SetTimer(Some(hwnd), DEBOUNCE_TIMER, DEBOUNCE_MS, None) };
            LRESULT(0)
        }
        WM_TIMER if wp.0 == DEBOUNCE_TIMER => {
            let _ = unsafe { KillTimer(Some(hwnd), DEBOUNCE_TIMER) };
            STATE.with(|s| {
                if let Some(state) = s.borrow_mut().as_mut() {
                    on_clipboard_changed(hwnd, state);
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// 打开剪贴板（带重试），Drop 时关闭
struct OpenedClipboard;

impl OpenedClipboard {
    fn open(owner: Option<HWND>) -> Option<Self> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(Self);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

impl Drop for OpenedClipboard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

fn read_global(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let hglobal = HGLOBAL(handle.0);
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, GlobalSize(hglobal)).to_vec();
        let _ = GlobalUnlock(hglobal);
        Some(bytes)
    }
}

fn read_text() -> Option<String> {
    let bytes = read_global(CF_UNICODETEXT)?;
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&c| c != 0)
        .collect();
    Some(String::from_utf16_lossy(&wide))
}

fn read_files() -> Option<Vec<String>> {
    unsafe {
        let handle = GetClipboardData(CF_HDROP).ok()?;
        let hdrop = HDROP(handle.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut files = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            files.push(String::from_utf16_lossy(&buf[..n]));
        }
        Some(files)
    }
}

fn read_image(formats: &Formats, available: &[u32]) -> Option<Payload> {
    // 优先用 PNG（浏览器等会提供，保留透明度），否则从 DIB 转换
    if available.contains(&formats.png)
        && let Some(png) = read_global(formats.png)
        && png.len() <= MAX_IMAGE_BYTES
        && let Some((width, height)) = png_dimensions(&png)
    {
        return Some(Payload::Image { png, width, height });
    }
    for fmt in [CF_DIBV5, CF_DIB] {
        if available.contains(&fmt)
            && let Some(dib) = read_global(fmt)
            && dib.len() <= MAX_IMAGE_BYTES
            && let Some((png, width, height)) = dib_to_png(&dib)
        {
            return Some(Payload::Image { png, width, height });
        }
    }
    None
}

fn owner_process(owner: HWND) -> Option<(u32, String)> {
    if owner.0.is_null() {
        return None;
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(owner, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        ok.ok()?;
        Some((pid, String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// 从 exe 的版本信息里读 FileDescription（例如 "Google Chrome"、"微信"），读不到就用文件名
fn app_display_name(exe: &str) -> Option<String> {
    let path = HSTRING::from(exe);
    let description = unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            None
        } else {
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(&path, None, size, data.as_mut_ptr() as *mut _).is_err() {
                None
            } else {
                // 先查语言代码页，再查 FileDescription
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                let translation = if VerQueryValueW(
                    data.as_ptr() as *const _,
                    w!("\\VarFileInfo\\Translation"),
                    &mut ptr,
                    &mut len,
                )
                .as_bool()
                    && len >= 4
                {
                    let pair = std::slice::from_raw_parts(ptr as *const u16, 2);
                    Some((pair[0], pair[1]))
                } else {
                    None
                };
                let mut candidates = Vec::new();
                if let Some((lang, cp)) = translation {
                    candidates.push(format!(
                        "\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"
                    ));
                }
                candidates.push("\\StringFileInfo\\040904b0\\FileDescription".into());
                candidates.push("\\StringFileInfo\\080404b0\\FileDescription".into());
                candidates.into_iter().find_map(|key| {
                    let key = HSTRING::from(key);
                    let mut ptr = std::ptr::null_mut();
                    let mut len = 0u32;
                    if VerQueryValueW(
                        data.as_ptr() as *const _,
                        PCWSTR(key.as_ptr()),
                        &mut ptr,
                        &mut len,
                    )
                    .as_bool()
                        && len > 1
                    {
                        let s = std::slice::from_raw_parts(ptr as *const u16, len as usize - 1);
                        let s = String::from_utf16_lossy(s).trim().to_string();
                        (!s.is_empty()).then_some(s)
                    } else {
                        None
                    }
                })
            }
        }
    };
    description.or_else(|| {
        Path::new(exe)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    })
}

fn on_clipboard_changed(hwnd: HWND, state: &mut WatcherState) {
    if state.paused.load(Ordering::Relaxed) {
        return;
    }
    let f = &state.formats;
    // 隐私标记：只需要 IsClipboardFormatAvailable，不用打开剪贴板，命中后连内容都不读
    let flagged = |fmt: u32| unsafe { IsClipboardFormatAvailable(fmt) }.is_ok();
    if flagged(f.exclude_monitor) || flagged(f.viewer_ignore) {
        return;
    }

    // 来源应用：剪贴板所有者窗口 → 进程；拿不到时退回前台窗口
    let owner = unsafe { GetClipboardOwner() }
        .ok()
        .and_then(owner_process)
        .or_else(|| owner_process(unsafe { GetForegroundWindow() }));
    let from_self = owner
        .as_ref()
        .is_some_and(|(pid, _)| *pid == unsafe { GetCurrentProcessId() });
    let source = owner.and_then(|(_, exe)| {
        state
            .app_names
            .entry(exe.clone())
            .or_insert_with(|| app_display_name(&exe))
            .clone()
    });

    let payload = {
        let Some(_guard) = OpenedClipboard::open(Some(hwnd)) else {
            return;
        };
        let mut available = Vec::new();
        let mut id = 0;
        loop {
            id = unsafe { EnumClipboardFormats(id) };
            if id == 0 {
                break;
            }
            available.push(id);
        }
        // CanIncludeInClipboardHistory = 0：调用方明确要求不进入历史
        if available.contains(&f.can_include_history)
            && read_global(f.can_include_history).and_then(|b| {
                b.get(..4)
                    .map(|d| u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
            }) == Some(0)
        {
            return;
        }

        // 优先级：文件 > 文本 > 图片（Office 复制表格会同时带位图，应当作文本）
        if available.contains(&CF_HDROP)
            && let Some(files) = read_files()
            && !files.is_empty()
        {
            Payload::Files(files)
        } else if available.contains(&CF_UNICODETEXT)
            && let Some(text) = read_text()
            && !text.trim().is_empty()
        {
            Payload::Text(text)
        } else if let Some(image) = read_image(f, &available) {
            image
        } else {
            return;
        }
    };

    (state.callback)(NewClip {
        payload,
        source,
        from_self,
    });
}

// ───────────────────────────── 写入 ─────────────────────────────

fn global_from_bytes(bytes: &[u8]) -> Result<HANDLE, String> {
    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| e.to_string())?;
        let ptr = GlobalLock(hmem) as *mut u8;
        if ptr.is_null() {
            return Err("GlobalLock failed".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(hmem);
        Ok(HANDLE(hmem.0))
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

/// CF_HDROP：DROPFILES 头（20 字节）+ 以 NUL 分隔、双 NUL 结尾的 UTF-16 路径
fn hdrop_bytes(files: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_le_bytes()); // pFiles：路径列表的偏移
    let pt = POINT::default();
    bytes.extend_from_slice(&pt.x.to_le_bytes());
    bytes.extend_from_slice(&pt.y.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes()); // fNC
    bytes.extend_from_slice(&1u32.to_le_bytes()); // fWide = TRUE
    for f in files {
        bytes.extend_from_slice(&utf16z(f));
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

pub fn write(owner: Option<isize>, content: WriteContent) -> Result<(), String> {
    // 先准备好所有数据，再打开剪贴板，尽量缩短占用时间
    let items: Vec<(u32, Vec<u8>)> = match content {
        WriteContent::Text(text) => vec![(CF_UNICODETEXT, utf16z(text))],
        WriteContent::Files(files) => {
            let text = files.join("\r\n");
            vec![
                (CF_HDROP, hdrop_bytes(&files)),
                (CF_UNICODETEXT, utf16z(&text)),
            ]
        }
        WriteContent::ImagePng(png) => {
            let mut items = vec![(reg("PNG"), png.to_vec())];
            if let Some(dib) = png_to_dib(png) {
                items.push((CF_DIB, dib));
            }
            items
        }
    };

    let owner = owner.map(|h| HWND(h as *mut _));
    let _guard = OpenedClipboard::open(owner).ok_or("剪贴板被其他程序占用")?;
    unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
    for (format, bytes) in items {
        let handle = global_from_bytes(&bytes)?;
        // 成功后内存归系统所有，不能再释放
        unsafe { SetClipboardData(format, Some(handle)) }.map_err(|e| e.to_string())?;
    }
    Ok(())
}
