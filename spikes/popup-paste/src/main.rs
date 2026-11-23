//! M0 技术验证 #2 / #5 / #6 + 界面原型：产品核心闭环。
//!
//! 启动后窗口隐藏，按 Alt+V 弹出：输入过滤 → ↑↓ 选择 → Enter 粘贴回之前的窗口；Esc 或点击外部隐藏。
//!
//! 测量与验证：
//! - #2 热键 → 首帧渲染的延迟；隐藏状态下的常驻内存
//! - #5 隐藏自身 → 切回之前的前台窗口 → SendInput Ctrl+V 的成功率与耗时
//! - #6 全局热键（global-hotkey）、PopUp 窗口（不进任务栏、置顶）、Mica 背景、圆角
//!
//! 环境变量：MYPASTE_DEBUG=1 打印按键和激活事件；MYPASTE_KIND=normal|popup；MYPASTE_BG=opaque|mica

#[cfg(windows)]
mod win;

mod data;
mod design;
mod ui;

use std::time::Instant;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::component::Theme;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// 带本地时间戳（东八区）的日志，方便和测试脚本的时间对齐
pub fn log_line(line: String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = (now.as_secs() + 8 * 3600) % 86400;
    println!(
        "{:02}:{:02}:{:02}.{:03} {line}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        now.subsec_millis()
    );
}

pub fn debug_log(line: String) {
    if std::env::var_os("MYPASTE_DEBUG").is_some() {
        log_line(line);
    }
}

fn main() {
    let (tx, rx) = async_channel::unbounded::<Instant>();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            // 必须在 gpui_kit::init 之后：同深度的键位绑定以后注册的为准
            ui::bind_keys(cx);
            Theme::sync_system_appearance(None, cx);

            // 全局热键：global-hotkey 在当前线程创建隐藏窗口接收 WM_HOTKEY，由 GPUI 的消息循环派发
            let manager = GlobalHotKeyManager::new().expect("hotkey manager");
            let hotkey = HotKey::new(Some(Modifiers::ALT), Code::KeyV);
            manager
                .register(hotkey)
                .expect("register Alt+V（可能已被其他程序占用）");
            let hotkey_id = hotkey.id();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                debug_log(format!("[hotkey] 回调 state={:?}", event.state));
                if event.id == hotkey_id && event.state == HotKeyState::Pressed {
                    let _ = tx.try_send(Instant::now());
                }
            }));
            // manager 析构会注销热键，让它活到进程结束
            std::mem::forget(manager);

            let kind = match std::env::var("MYPASTE_KIND").as_deref() {
                Ok("normal") => WindowKind::Normal,
                _ => WindowKind::PopUp,
            };
            let background = match std::env::var("MYPASTE_BG").as_deref() {
                Ok("opaque") => WindowBackgroundAppearance::Opaque,
                _ => WindowBackgroundAppearance::MicaBackdrop,
            };

            let bounds =
                Bounds::centered(None, size(px(design::WINDOW_W), px(design::WINDOW_H)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                kind,
                // 以 show: true 创建，在构建闭包里立刻用 Win32 隐藏（首帧之前，不会闪烁）。
                // 如果用 show: false，GPUI 会保存"延迟应用的位置状态"，在第一次 activate 时异步应用，
                // 打乱我们自己的显示和焦点流程。
                show: true,
                focus: false,
                is_resizable: false,
                window_background: background,
                ..Default::default()
            };

            let (window_handle, popup) = gpui_kit::open_window(options, cx, |window, cx| {
                let hwnd = match window.window_handle().map(|h| h.as_raw()) {
                    Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                    _ => panic!("expected a Win32 window handle"),
                };
                win::hide(hwnd);
                win::round_corners(hwnd);
                cx.new(|cx| ui::Popup::new(hwnd, window, cx))
            })
            .expect("failed to open window");

            let (ws, private) = win::memory_mb();
            log_line(format!(
                "[ready] 按 Alt+V 唤起 · 启动后工作集 {ws:.1}MB · 私有 {private:.1}MB"
            ));

            cx.spawn(async move |cx| {
                while let Ok(t0) = rx.recv().await {
                    let popup = popup.clone();
                    let _ = window_handle.update(cx, |_, window, cx| {
                        popup.update(cx, |this, cx| this.toggle(t0, window, cx));
                    });
                }
            })
            .detach();
        });
}
