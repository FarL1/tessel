//! Tessel · 拼贴 —— 原生、键盘优先的剪贴板工具（Windows 桌面端）。
//!
//! 双击启动时直接弹出主界面；之后常驻托盘，按热键（默认 Alt+V）唤起：
//! 输入过滤 → ↑↓ 选择 → Enter 粘贴回之前的窗口；Esc 或点击外部隐藏。
//!
//! 命令行参数：`--background` 启动后不弹出（开机自启用）。
//! 环境变量：TESSEL_DEBUG=1 打印详细日志；TESSEL_DATA_DIR 指定数据目录（测试用）；
//! TESSEL_KIND=normal|popup；TESSEL_BG=opaque|mica

// release 构建用 GUI 子系统：双击 exe 时不再额外弹出控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

mod design;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tessel_core::{Config, Payload, Store};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
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

/// 从热键、托盘、采集线程、第二个实例等各处汇集到 GPUI 主线程的命令
#[derive(Clone, Copy, Debug)]
pub enum AppCommand {
    /// 热键：显示 ↔ 隐藏
    Toggle(Instant),
    /// 托盘、再次启动：只显示（已经显示就保持）
    Show(Instant),
    /// 采集线程写入了新内容
    ClipboardChanged,
    /// 配置文件被修改
    ConfigChanged,
    ResetPosition,
    OpenSettings,
    /// 托盘复选项被点击（muda 已经切换了勾选状态）
    AutostartToggled,
    PauseToggled,
    ClearHistory,
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

/// 解析热键字符串。在 global-hotkey 的语法上额外接受 "Win"。
fn parse_hotkey(s: &str) -> Result<HotKey, String> {
    let normalized: Vec<String> = s
        .split('+')
        .map(|t| match t.trim().to_ascii_uppercase().as_str() {
            "WIN" | "WINDOWS" => "Super".to_string(),
            _ => t.trim().to_string(),
        })
        .collect();
    normalized
        .join("+")
        .parse::<HotKey>()
        .map_err(|e| format!("热键「{s}」格式不对：{e}"))
}

/// 托盘里需要在运行时更新的菜单项
struct TrayItems {
    show: MenuItem,
    pause: CheckMenuItem,
    autostart: CheckMenuItem,
}

/// 托盘图标和菜单。对象析构时托盘图标会消失，所以要保存到进程结束。
fn build_tray(
    tx: async_channel::Sender<AppCommand>,
    hotkey_label: &str,
) -> Option<(tray_icon::TrayIcon, TrayItems)> {
    let show = MenuItem::new(format!("显示 Tessel\t{hotkey_label}"), true, None);
    let pause = CheckMenuItem::new("暂停记录", true, false, None);
    let autostart = CheckMenuItem::new("开机自动启动", true, win::autostart_enabled(), None);
    let settings = MenuItem::new("打开设置文件…", true, None);
    let reset = MenuItem::new("重置窗口位置", true, None);
    let clear = MenuItem::new("清空历史（保留收藏）…", true, None);
    let quit = MenuItem::new("退出", true, None);
    let sep = PredefinedMenuItem::separator;
    let menu = Menu::new();
    menu.append_items(&[
        &show,
        &sep(),
        &pause,
        &autostart,
        &settings,
        &reset,
        &sep(),
        &clear,
        &sep(),
        &quit,
    ])
    .ok()?;

    let routes = [
        (show.id().clone(), None),
        (pause.id().clone(), Some(AppCommand::PauseToggled)),
        (autostart.id().clone(), Some(AppCommand::AutostartToggled)),
        (settings.id().clone(), Some(AppCommand::OpenSettings)),
        (reset.id().clone(), Some(AppCommand::ResetPosition)),
        (clear.id().clone(), Some(AppCommand::ClearHistory)),
        (quit.id().clone(), Some(AppCommand::Quit)),
    ];
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some((_, cmd)) = routes.iter().find(|(id, _)| *id == event.id) {
            let _ = menu_tx.try_send(cmd.unwrap_or(AppCommand::Show(Instant::now())));
        }
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
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip(format!("{APP_NAME} {APP_NAME_ZH} · {hotkey_label} 唤起"))
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()?;
    Some((
        tray,
        TrayItems {
            show,
            pause,
            autostart,
        },
    ))
}

/// 每秒检查一次配置文件的修改时间，变了就通知主线程重新加载
fn watch_config(path: PathBuf, tx: async_channel::Sender<AppCommand>) {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    std::thread::Builder::new()
        .name("tessel-config-watcher".into())
        .spawn(move || {
            let mut last: Option<SystemTime> = mtime(&path);
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let now = mtime(&path);
                if now != last {
                    last = now;
                    if tx.try_send(AppCommand::ConfigChanged).is_err() {
                        break;
                    }
                }
            }
        })
        .ok();
}

/// 全局热键：按配置注册，配置变化时换绑
struct Hotkeys {
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
    /// 给热键回调用的当前热键 id（回调在其他上下文里执行，不能直接访问 `current`）
    active_id: Arc<AtomicU32>,
}

impl Hotkeys {
    fn apply(&mut self, spec: &str) -> Result<(), String> {
        let hotkey = parse_hotkey(spec)?;
        if self.current == Some(hotkey) {
            return Ok(());
        }
        // 先注册新的，成功后再注销旧的：新热键被占用时旧热键继续可用
        self.manager
            .register(hotkey)
            .map_err(|_| format!("热键「{spec}」已被其他程序占用，请在设置里换一个"))?;
        if let Some(old) = self.current.take() {
            let _ = self.manager.unregister(old);
        }
        self.current = Some(hotkey);
        self.active_id.store(hotkey.id(), Ordering::Relaxed);
        Ok(())
    }
}

fn main() {
    // 单实例：如果已经有一个在运行，通知它显示出来，然后自己退出
    let Some(show_requests) = win::acquire_single_instance() else {
        return;
    };
    let start_hidden = std::env::args().any(|a| a == "--background");

    let (tx, rx) = async_channel::unbounded::<AppCommand>();

    let dir = data_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log_line(format!("[error] 无法创建数据目录 {}：{err}", dir.display()));
        return;
    }
    log_line(format!("[store] 数据目录 {}", dir.display()));

    // 配置：读取失败时用默认值启动，并在界面上提示
    let (config, config_error) = match Config::load_or_create(&dir) {
        Ok(config) => (config, None),
        Err(err) => (Config::default(), Some(format!("{err}（已使用默认设置）"))),
    };
    let shared_config = Arc::new(RwLock::new(config.clone()));
    watch_config(Config::path(&dir), tx.clone());

    // 存储：UI 线程和采集线程各用一个连接（SQLite WAL 模式支持并发读写）
    let ui_store = match Store::open(&dir) {
        Ok(store) => store,
        Err(err) => {
            log_line(format!("[error] 无法打开数据库：{err}"));
            return;
        }
    };

    // 剪贴板采集线程
    let paused = Arc::new(AtomicBool::new(false));
    match Store::open(&dir) {
        Ok(store) => {
            let store = std::cell::RefCell::new(store);
            let tx = tx.clone();
            let ignore_cfg = shared_config.clone();
            let keep_cfg = shared_config.clone();
            let result = tessel_clipboard::spawn_watcher(
                paused.clone(),
                move |exe| ignore_cfg.read().is_ok_and(|c| c.is_ignored(exe)),
                move |clip| {
                    let kind = match &clip.payload {
                        Payload::Text(_) => "文本",
                        Payload::Files(_) => "文件",
                        Payload::Image { .. } => "图片",
                    };
                    let source = clip.source.clone().unwrap_or_default();
                    let mut store = store.borrow_mut();
                    if let Ok(cfg) = keep_cfg.read() {
                        store.set_keep(cfg.max_items);
                    }
                    match store.record(clip) {
                        Ok(Some(seq)) => {
                            debug_log(format!("[capture] {kind} · {source} · seq={seq}"));
                            let _ = tx.try_send(AppCommand::ClipboardChanged);
                        }
                        Ok(None) => {}
                        Err(err) => log_line(format!("[capture] 写入失败：{err}")),
                    }
                },
            );
            if let Err(err) = result {
                log_line(format!("[error] 启动剪贴板监听失败：{err}"));
            }
        }
        Err(err) => log_line(format!("[error] 采集线程无法打开数据库：{err}")),
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

            // 全局热键：global-hotkey 在当前线程创建隐藏窗口接收 WM_HOTKEY，由 GPUI 的消息循环派发
            let mut hotkeys = Hotkeys {
                manager: GlobalHotKeyManager::new().expect("hotkey manager"),
                current: None,
                active_id: Arc::new(AtomicU32::new(0)),
            };
            let mut startup_error = config_error;
            if let Err(err) = hotkeys.apply(&config.hotkey) {
                log_line(format!("[warn] {err}"));
                startup_error.get_or_insert(err);
            }
            let hotkey_tx = tx.clone();
            let active_id = hotkeys.active_id.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                debug_log(format!("[hotkey] 回调 state={:?}", event.state));
                if event.id == active_id.load(Ordering::Relaxed)
                    && event.state == HotKeyState::Pressed
                {
                    let _ = hotkey_tx.try_send(AppCommand::Toggle(Instant::now()));
                }
            }));

            // 托盘：tray-icon 同样依赖当前线程的消息循环（GPUI 的）
            let (tray, tray_items) = match build_tray(tx.clone(), &config.hotkey) {
                Some((tray, items)) => (Some(tray), Some(items)),
                None => {
                    log_line("[warn] 创建托盘图标失败".into());
                    (None, None)
                }
            };

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

            // 界面里切换"暂停记录"时，同步托盘里的勾选状态
            let pause_item = tray_items.as_ref().map(|t| t.pause.clone());
            let on_pause_changed: Box<dyn Fn(bool)> = Box::new(move |paused| {
                if let Some(item) = &pause_item {
                    item.set_checked(paused);
                }
            });

            let ui_config = config.clone();
            let (window_handle, popup) = gpui_kit::open_window(options, cx, |window, cx| {
                let hwnd = match window.window_handle().map(|h| h.as_raw()) {
                    Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                    _ => panic!("expected a Win32 window handle"),
                };
                win::hide(hwnd);
                win::round_corners(hwnd);
                win::set_title(hwnd, &format!("{APP_NAME} {APP_NAME_ZH}"));
                cx.new(|cx| {
                    ui::Popup::new(
                        hwnd,
                        ui_store,
                        paused.clone(),
                        ui_config,
                        on_pause_changed,
                        window,
                        cx,
                    )
                })
            })
            .expect("failed to open window");

            if let Some(err) = startup_error {
                popup.update(cx, |this, cx| this.set_config_error(err, cx));
            }

            let (ws, private) = win::memory_mb();
            log_line(format!(
                "[ready] 按 {} 唤起 · 启动后工作集 {ws:.1}MB · 私有 {private:.1}MB",
                config.hotkey
            ));

            // 双击启动时直接弹出，让用户知道它已经在运行
            if !start_hidden {
                let _ = tx.try_send(AppCommand::Show(Instant::now()));
            }

            let config_path = Config::path(&dir);
            cx.spawn(async move |cx| {
                // 托盘对象必须活到进程结束
                let _tray = tray;
                while let Ok(cmd) = rx.recv().await {
                    match cmd {
                        AppCommand::Quit => {
                            cx.update(|cx| cx.quit());
                            break;
                        }
                        AppCommand::ConfigChanged => {
                            let result = Config::load_or_create(&dir).and_then(|new| {
                                hotkeys.apply(&new.hotkey).map(|_| new)
                            });
                            let _ = window_handle.update(cx, |_, window, cx| {
                                popup.update(cx, |this, cx| match result {
                                    Ok(new) => {
                                        log_line(format!("[config] 已重新加载（热键 {}）", new.hotkey));
                                        if let Some(items) = &tray_items {
                                            items
                                                .show
                                                .set_text(format!("显示 Tessel\t{}", new.hotkey));
                                        }
                                        if let Ok(mut shared) = shared_config.write() {
                                            *shared = new.clone();
                                        }
                                        this.apply_config(new, window, cx);
                                        this.set_status("设置已更新".into(), cx);
                                    }
                                    Err(err) => {
                                        log_line(format!("[config] {err}"));
                                        this.set_config_error(err, cx);
                                    }
                                });
                            });
                        }
                        AppCommand::OpenSettings => {
                            // 记事本一定存在；.toml 不一定关联了编辑器
                            if let Err(err) = std::process::Command::new("notepad.exe")
                                .arg(&config_path)
                                .spawn()
                            {
                                log_line(format!("[error] 无法打开设置文件：{err}"));
                            }
                        }
                        AppCommand::AutostartToggled => {
                            if let Some(items) = &tray_items {
                                let want = items.autostart.is_checked();
                                if let Err(err) = win::set_autostart(want) {
                                    log_line(format!("[error] 设置开机自启失败：{err}"));
                                    items.autostart.set_checked(!want);
                                }
                            }
                        }
                        AppCommand::PauseToggled => {
                            if let Some(items) = &tray_items {
                                let want = items.pause.is_checked();
                                popup.update(cx, |this, cx| this.set_paused(want, cx));
                            }
                        }
                        AppCommand::ClearHistory => {
                            if win::confirm(
                                "清空历史 · Tessel",
                                "确定要清空所有未收藏的剪贴板历史吗？\n收藏的内容会保留。此操作无法撤销。",
                            ) {
                                popup.update(cx, |this, cx| this.clear_history(cx));
                            }
                        }
                        cmd => {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                popup.update(cx, |this, cx| match cmd {
                                    AppCommand::Toggle(t0) => this.toggle(t0, window, cx),
                                    AppCommand::Show(t0) => this.show_if_hidden(t0, window, cx),
                                    AppCommand::ClipboardChanged => this.on_clipboard_changed(cx),
                                    AppCommand::ResetPosition => this.reset_position(),
                                    _ => {}
                                });
                            });
                        }
                    }
                }
            })
            .detach();
        });
}
