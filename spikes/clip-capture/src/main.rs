//! M0 技术验证 #4：Windows 剪贴板事件驱动采集。
//!
//! 运行：`cargo run -p spike-clip-capture`，然后在任意应用里复制内容，观察控制台输出。
//!
//! 验证点：
//! - `AddClipboardFormatListener` + 隐藏消息窗口：事件驱动，空闲时 CPU 为 0
//! - 列出每次复制带来的全部格式（只列名字，**不**读取数据：读取会触发源应用的延迟渲染，开销可能很大）
//! - 只读取我们关心的格式：文本、HTML、RTF、PNG、DIBV5、文件列表
//! - 隐私标记：`ExcludeClipboardContentFromMonitorProcessing`、`CanIncludeInClipboardHistory = 0`
//!   命中时连内容都不读
//! - 来源应用：剪贴板所有者窗口 → 进程 exe 路径
//! - 从收到通知到读完数据的耗时；剪贴板被其他程序占用时的重试次数

#[cfg(not(windows))]
fn main() {
    eprintln!("this spike only runs on Windows");
}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    win::run()
}

#[cfg(windows)]
mod win {
    use std::cell::Cell;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, CloseClipboard, EnumClipboardFormats, GetClipboardData,
        GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber,
        IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
        GetWindowThreadProcessId, HWND_MESSAGE, KillTimer, MSG, RegisterClassW, SetTimer,
        WINDOW_EX_STYLE, WM_CLIPBOARDUPDATE, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
    };
    use windows::core::{HSTRING, PWSTR, w};

    // 标准剪贴板格式（winuser.h）
    const CF_TEXT: u32 = 1;
    const CF_BITMAP: u32 = 2;
    const CF_METAFILEPICT: u32 = 3;
    const CF_OEMTEXT: u32 = 7;
    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const CF_ENHMETAFILE: u32 = 14;
    const CF_HDROP: u32 = 15;
    const CF_LOCALE: u32 = 16;
    const CF_DIBV5: u32 = 17;

    /// 运行时注册的自定义格式 ID
    struct Formats {
        html: u32,
        rtf: u32,
        png: u32,
        exclude_monitor: u32,
        can_include_history: u32,
        can_upload_cloud: u32,
    }

    static FORMATS: OnceLock<Formats> = OnceLock::new();

    /// 合并窗口：很多应用写剪贴板分两步（先 Set 再 Flush），会连续触发两次通知。
    /// 收到通知后等一小段时间再读，既能合并重复通知，也避开了源应用还占着剪贴板的时刻。
    const DEBOUNCE_MS: u32 = 40;
    const DEBOUNCE_TIMER_ID: usize = 1;

    thread_local! {
        static EVENT_NO: Cell<u64> = const { Cell::new(0) };
        static LAST_SEQ: Cell<u32> = const { Cell::new(0) };
        /// 当前合并窗口内收到的通知次数
        static PENDING_NOTIFIES: Cell<u32> = const { Cell::new(0) };
    }

    fn reg(name: &str) -> u32 {
        unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) }
    }

    pub fn run() -> windows::core::Result<()> {
        FORMATS.get_or_init(|| Formats {
            html: reg("HTML Format"),
            rtf: reg("Rich Text Format"),
            png: reg("PNG"),
            exclude_monitor: reg("ExcludeClipboardContentFromMonitorProcessing"),
            can_include_history: reg("CanIncludeInClipboardHistory"),
            can_upload_cloud: reg("CanUploadToCloudClipboard"),
        });

        unsafe {
            let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = w!("MyPasteClipSpike");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: class_name,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            // HWND_MESSAGE：仅接收消息的隐藏窗口，不出现在任务栏、Alt+Tab 中
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("MyPaste clipboard listener"),
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

            println!("正在监听剪贴板（Ctrl+C 退出）……去任意应用里复制点东西试试\n");

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_CLIPBOARDUPDATE => {
                PENDING_NOTIFIES.with(|n| n.set(n.get() + 1));
                // 重复调用 SetTimer 会重置计时：连续通知只在最后一次之后处理一次
                unsafe { SetTimer(Some(hwnd), DEBOUNCE_TIMER_ID, DEBOUNCE_MS, None) };
                return LRESULT(0);
            }
            WM_TIMER if wp.0 == DEBOUNCE_TIMER_ID => {
                let _ = unsafe { KillTimer(Some(hwnd), DEBOUNCE_TIMER_ID) };
                on_clipboard_update(hwnd);
                return LRESULT(0);
            }
            _ => {}
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    /// 打开剪贴板，带重试（其他程序可能短暂占用剪贴板）。Drop 时自动关闭。
    struct OpenedClipboard;

    impl OpenedClipboard {
        fn open(hwnd: HWND) -> Option<(Self, u32)> {
            for attempt in 0..10 {
                if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                    return Some((Self, attempt));
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

    fn format_name(id: u32) -> String {
        let std_name = match id {
            CF_TEXT => "CF_TEXT",
            CF_BITMAP => "CF_BITMAP",
            CF_METAFILEPICT => "CF_METAFILEPICT",
            CF_OEMTEXT => "CF_OEMTEXT",
            CF_DIB => "CF_DIB",
            CF_UNICODETEXT => "CF_UNICODETEXT",
            CF_ENHMETAFILE => "CF_ENHMETAFILE",
            CF_HDROP => "CF_HDROP",
            CF_LOCALE => "CF_LOCALE",
            CF_DIBV5 => "CF_DIBV5",
            _ => "",
        };
        if !std_name.is_empty() {
            return std_name.to_string();
        }
        let mut buf = [0u16; 256];
        let n = unsafe { GetClipboardFormatNameW(id, &mut buf) };
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            format!("#{id}")
        }
    }

    /// 读取 HGLOBAL 类格式的原始字节。调用前剪贴板必须已打开。
    fn read_global(format: u32) -> Option<Vec<u8>> {
        unsafe {
            let handle = GetClipboardData(format).ok()?;
            let hglobal = HGLOBAL(handle.0);
            let ptr = GlobalLock(hglobal) as *const u8;
            if ptr.is_null() {
                return None;
            }
            let size = GlobalSize(hglobal);
            let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
            // 锁计数归零时 GlobalUnlock 会返回"错误"（实际是 NO_ERROR），忽略即可
            let _ = GlobalUnlock(hglobal);
            Some(bytes)
        }
    }

    fn read_unicode_text() -> Option<String> {
        let bytes = read_global(CF_UNICODETEXT)?;
        let wide: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
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

    fn process_path_of_window(hwnd: HWND) -> Option<String> {
        if hwnd.0.is_null() {
            return None;
        }
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let process: HANDLE =
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(process);
            result.ok()?;
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    fn human_size(n: usize) -> String {
        match n {
            0..1024 => format!("{n}B"),
            1024..1_048_576 => format!("{:.1}KB", n as f64 / 1024.0),
            _ => format!("{:.1}MB", n as f64 / 1_048_576.0),
        }
    }

    fn preview(s: &str, max_chars: usize) -> String {
        let one_line: String = s
            .chars()
            .map(|c| if c.is_control() { '⏎' } else { c })
            .take(max_chars)
            .collect();
        if s.chars().count() > max_chars {
            format!("{one_line}…")
        } else {
            one_line
        }
    }

    fn on_clipboard_update(hwnd: HWND) {
        let t0 = Instant::now();
        let f = FORMATS.get().expect("formats registered");
        let seq = unsafe { GetClipboardSequenceNumber() };

        // 同一序列号重复通知时跳过（部分应用会连续触发）
        if LAST_SEQ.with(|s| s.replace(seq)) == seq {
            PENDING_NOTIFIES.with(|n| n.set(0));
            return;
        }
        let no = EVENT_NO.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });

        // 来源应用：优先用剪贴板所有者，拿不到时退回前台窗口
        let owner = unsafe { GetClipboardOwner() }.ok();
        let source = owner
            .and_then(process_path_of_window)
            .map(|p| format!("{p}（所有者窗口）"))
            .or_else(|| {
                process_path_of_window(unsafe { GetForegroundWindow() })
                    .map(|p| format!("{p}（前台窗口，兜底）"))
            })
            .unwrap_or_else(|| "未知".into());

        // 隐私标记检查：只需 IsClipboardFormatAvailable，无需打开剪贴板
        let excluded = unsafe { IsClipboardFormatAvailable(f.exclude_monitor) }.is_ok();

        let Some((_guard, retries)) = OpenedClipboard::open(hwnd) else {
            println!("#{no} seq={seq} ⚠ 无法打开剪贴板（被其他程序长时间占用）\n");
            return;
        };

        // 枚举全部格式（只拿 ID，不读数据）
        let mut formats = Vec::new();
        let mut id = 0;
        loop {
            id = unsafe { EnumClipboardFormats(id) };
            if id == 0 {
                break;
            }
            formats.push(id);
        }

        let history_allowed = if formats.contains(&f.can_include_history) {
            read_global(f.can_include_history)
                .and_then(|b| {
                    b.get(..4)
                        .map(|d| u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
                })
                .map(|v| v != 0)
        } else {
            None
        };
        let cloud_allowed = if formats.contains(&f.can_upload_cloud) {
            read_global(f.can_upload_cloud)
                .and_then(|b| {
                    b.get(..4)
                        .map(|d| u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
                })
                .map(|v| v != 0)
        } else {
            None
        };

        let merged = PENDING_NOTIFIES.with(|n| n.replace(0));
        println!("━━ #{no}  seq={seq}  来源：{source}  （合并了 {merged} 次通知）");
        println!(
            "   格式（{}）：{}",
            formats.len(),
            formats
                .iter()
                .map(|&id| format_name(id))
                .collect::<Vec<_>>()
                .join(" · ")
        );

        if excluded || history_allowed == Some(false) {
            let reason = if excluded {
                "ExcludeClipboardContentFromMonitorProcessing"
            } else {
                "CanIncludeInClipboardHistory = 0"
            };
            println!("   🔒 隐私标记：{reason} → 跳过，不读取内容");
            println!("   耗时 {:.2}ms\n", t0.elapsed().as_secs_f64() * 1000.0);
            return;
        }

        // 只读取我们关心的格式
        if let Some(text) = read_unicode_text() {
            println!(
                "   文本：{} 字符 「{}」",
                text.chars().count(),
                preview(&text, 60)
            );
        }
        for (fmt, label) in [
            (f.html, "HTML"),
            (f.rtf, "RTF"),
            (f.png, "PNG"),
            (CF_DIBV5, "DIBV5"),
        ] {
            if formats.contains(&fmt)
                && let Some(b) = read_global(fmt)
            {
                println!("   {label}：{}", human_size(b.len()));
            }
        }
        if formats.contains(&CF_HDROP)
            && let Some(files) = read_files()
        {
            println!("   文件 {} 个：", files.len());
            for file in files.iter().take(5) {
                println!("     - {file}");
            }
        }
        if let Some(v) = cloud_allowed {
            println!("   CanUploadToCloudClipboard = {v}");
        }

        println!(
            "   耗时 {:.2}ms（打开剪贴板重试 {retries} 次）\n",
            t0.elapsed().as_secs_f64() * 1000.0
        );
    }
}
