//! Tessel · 拼贴 —— 原型（M0 技术验证 #2 / #5 / #6 + 界面原型）。
//!
//! 双击启动时直接弹出主界面；之后常驻托盘，按 Alt+V 唤起：
//! 输入过滤 → ↑↓ 选择 → Enter 粘贴回之前的窗口；Esc 或点击外部隐藏。
//!
//! 命令行参数：`--background` 启动后不弹出（给开机自启用）。
//! 环境变量：TESSEL_DEBUG=1 打印按键和激活事件；TESSEL_KIND=normal|popup；TESSEL_BG=opaque|mica

// release 构建用 GUI 子系统：双击 exe 时不再额外弹出控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

mod design;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::component::Theme;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton as TrayButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

pub const APP_NAME: &str = "Tessel";
pub const APP_NAME_ZH: &str = "拼贴";

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
    if std::env::var_os("TESSEL_DEBUG").is_some() {
        log_line(line);
    }
}

/// 从热键、托盘、第二个实例等各处汇集到 GPUI 主线程的命令
#[derive(Clone, Copy, Debug)]
pub enum AppCommand {
    /// 热键：显示 ↔ 隐藏
    Toggle(Instant),
    /// 托盘、再次启动：只显示（已经显示就保持）
    Show(Instant),
    /// 采集线程写入了新内容
    ClipboardChanged,
    /// 托盘菜单：窗口回到默认位置
    ResetPosition,
    Quit,
}

/// 数据目录：默认 %LOCALAPPDATA%\Tessel；可以用 TESSEL_DATA_DIR 指定（测试时用，避免混进真实历史）
fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TESSEL_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Tessel")
}

/// 托盘图标和菜单。对象析构时托盘图标会消失，所以要保存到进程结束。
fn build_tray(tx: async_channel::Sender<AppCommand>) -> Option<tray_icon::TrayIcon> {
    let show_item = MenuItem::new("显示 Tessel\tAlt+V", true, None);
    let reset_item = MenuItem::new("重置窗口位置", true, None);
    let quit_item = MenuItem::new("退出", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &show_item,
        &reset_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])
    .ok()?;

    let (show_id, reset_id, quit_id) = (
        show_item.id().clone(),
        reset_item.id().clone(),
        quit_item.id().clone(),
    );
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let cmd = if event.id == show_id {
            AppCommand::Show(Instant::now())
        } else if event.id == reset_id {
            AppCommand::ResetPosition
        } else if event.id == quit_id {
            AppCommand::Quit
        } else {
            return;
        };
        let _ = menu_tx.try_send(cmd);
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: TrayButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = tx.try_send(AppCommand::Show(Instant::now()));
        }
    }));

    // 图标从 exe 内嵌的资源（tessel.rc 里的序号 1）加载
    let icon = tray_icon::Icon::from_resource(1, Some((32, 32))).ok()?;
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip(format!("{APP_NAME} {APP_NAME_ZH} · Alt+V 唤起"))
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()
}

fn main() {
    // 单实例：如果已经有一个在运行，通知它显示出来，然后自己退出
    let Some(show_requests) = win::acquire_single_instance() else {
        return;
    };
    let start_hidden = std::env::args().any(|a| a == "--background");

    let (tx, rx) = async_channel::unbounded::<AppCommand>();

    // 存储：UI 线程和采集线程各用一个连接（SQLite WAL 模式支持并发读写）
    let dir = data_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log_line(format!("[error] 无法创建数据目录 {}：{err}", dir.display()));
        return;
    }
    let ui_store = match tessel_core::Store::open(&dir) {
        Ok(store) => store,
        Err(err) => {
            log_line(format!("[error] 无法打开数据库：{err}"));
            return;
        }
    };
    log_line(format!("[store] 数据目录 {}", dir.display()));

    // 剪贴板采集线程
    let paused = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let store = tessel_core::Store::open(&dir).map(std::cell::RefCell::new);
        match store {
            Ok(store) => {
                let result = tessel_clipboard::spawn_watcher(paused.clone(), move |clip| {
                    let kind = match &clip.payload {
                        tessel_core::Payload::Text(_) => "文本",
                        tessel_core::Payload::Files(_) => "文件",
                        tessel_core::Payload::Image { .. } => "图片",
                    };
                    let source = clip.source.clone().unwrap_or_default();
                    match store.borrow_mut().record(clip) {
                        Ok(Some(seq)) => {
                            debug_log(format!("[capture] {kind} · {source} · seq={seq}"));
                            let _ = tx.try_send(AppCommand::ClipboardChanged);
                        }
                        Ok(None) => {}
                        Err(err) => log_line(format!("[capture] 写入失败：{err}")),
                    }
                });
                if let Err(err) = result {
                    log_line(format!("[error] 启动剪贴板监听失败：{err}"));
                }
            }
            Err(err) => log_line(format!("[error] 采集线程无法打开数据库：{err}")),
        }
    }

    // 第二个实例的"显示"请求
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while show_requests.wait() {
                let _ = tx.try_send(AppCommand::Show(Instant::now()));
            }
        });
    }

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
            if let Err(err) = manager.register(hotkey) {
                log_line(format!(
                    "[warn] 注册 Alt+V 失败（可能已被其他程序占用）：{err}"
                ));
            }
            let hotkey_id = hotkey.id();
            let hotkey_tx = tx.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                debug_log(format!("[hotkey] 回调 state={:?}", event.state));
                if event.id == hotkey_id && event.state == HotKeyState::Pressed {
                    let _ = hotkey_tx.try_send(AppCommand::Toggle(Instant::now()));
                }
            }));
            // manager 析构会注销热键，让它活到进程结束
            std::mem::forget(manager);

            // 托盘：tray-icon 同样依赖当前线程的消息循环（GPUI 的）
            match build_tray(tx.clone()) {
                Some(tray) => std::mem::forget(tray),
                None => log_line("[warn] 创建托盘图标失败".into()),
            }

            let kind = match std::env::var("TESSEL_KIND").as_deref() {
                Ok("normal") => WindowKind::Normal,
                _ => WindowKind::PopUp,
            };
            let background = match std::env::var("TESSEL_BG").as_deref() {
                Ok("opaque") => WindowBackgroundAppearance::Opaque,
                _ => WindowBackgroundAppearance::MicaBackdrop,
            };

            let bounds =
                Bounds::centered(None, size(px(design::WINDOW_W), px(design::WINDOW_H)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // 不要设置 GPUI 的 titlebar：一旦设置，GPUI 就会画出一条系统标题栏。
                // 窗口标题（任务管理器、辅助功能里显示的名字）改用 Win32 设置。
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
                win::set_title(hwnd, &format!("{APP_NAME} {APP_NAME_ZH}"));
                cx.new(|cx| ui::Popup::new(hwnd, ui_store, paused, window, cx))
            })
            .expect("failed to open window");

            let (ws, private) = win::memory_mb();
            log_line(format!(
                "[ready] 按 Alt+V 唤起 · 启动后工作集 {ws:.1}MB · 私有 {private:.1}MB"
            ));

            // 双击启动时直接弹出，让用户知道它已经在运行
            if !start_hidden {
                let _ = tx.try_send(AppCommand::Show(Instant::now()));
            }

            cx.spawn(async move |cx| {
                while let Ok(cmd) = rx.recv().await {
                    if let AppCommand::Quit = cmd {
                        cx.update(|cx| cx.quit());
                        break;
                    }
                    let popup = popup.clone();
                    let _ = window_handle.update(cx, |_, window, cx| {
                        popup.update(cx, |this, cx| match cmd {
                            AppCommand::Toggle(t0) => this.toggle(t0, window, cx),
                            AppCommand::Show(t0) => this.show_if_hidden(t0, window, cx),
                            AppCommand::ClipboardChanged => this.on_clipboard_changed(cx),
                            AppCommand::ResetPosition => this.reset_position(),
                            AppCommand::Quit => {}
                        });
                    });
                }
            })
            .detach();
        });
}
