//! Tessel · 拼贴 —— 原生、键盘优先的剪贴板工具（Windows 桌面端）。
//!
//! 双击启动时直接弹出主界面；之后常驻托盘，按热键（默认 Alt+V）唤起：
//! 输入过滤 → ↑↓ 选择 → Enter 粘贴回之前的窗口；Esc 或点击外部隐藏。
//!
//! 命令行参数：`--background` 启动后不弹出（开机自启用）；`--settings` 启动后打开设置窗口。
//! 环境变量：TESSEL_DEBUG=1 打印详细日志；TESSEL_DATA_DIR 指定数据目录（测试用）；
//! TESSEL_KIND=normal|popup；TESSEL_BG=opaque|mica

// release 构建用 GUI 子系统：双击 exe 时不再额外弹出控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

mod action_menu;
mod design;
#[cfg(windows)]
mod ocr;
mod settings;
mod toast;
mod ui;
mod update;
mod welcome;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tessel_core::i18n::{self, tr};
use tessel_core::{Config, Payload, Store, trf};
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
    /// 采集线程写入了新内容。`notify`：是用户自己复制的（不是 Tessel 写回的），要提示"已复制"
    ClipboardChanged {
        notify: bool,
    },
    /// 粘贴队列热键：粘贴队列里的下一条（带上按下热键的时刻，用来记录延迟）
    PasteNext(Instant),
    /// 配置文件被修改
    ConfigChanged,
    ResetPosition,
    /// 打开设置窗口（已经打开就切到前台）
    OpenSettingsWindow,
    /// 设置窗口的"返回"：先显示弹窗，再关闭设置窗口
    BackToPopup,
    /// 设置里点了"立即检查"
    CheckUpdates,
    /// 后台检查更新完成（结果在 `UpdateSlot` 里）。`manual`：是用户点的"立即检查"
    UpdateChecked {
        manual: bool,
    },
    /// 托盘菜单"下载新版本"
    OpenReleasePage,
    /// 安装完成页点了"完成"：是否现在启动、是否开机自启
    FinishSetup {
        launch: bool,
        autostart: bool,
    },
    /// 用记事本打开 config.toml
    OpenConfigFile,
    /// 托盘复选项被点击（muda 已经切换了勾选状态）
    AutostartToggled,
    /// 设置窗口里切换开机自启
    SetAutostart(bool),
    /// 设置窗口开始 / 结束录制热键：期间暂时注销全局热键
    HotkeyRecording(bool),
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
        .map_err(|e| trf!("热键「{s}」格式不对：{e}", "Invalid hotkey \"{s}\": {e}"))
}

/// 设置窗口的句柄和视图（弱引用：窗口关闭后视图被释放）
type SettingsWindow = (AnyWindowHandle, WeakEntity<settings::SettingsView>);

/// 托盘里需要在运行时更新的菜单项
/// 最近一次检查更新的结果（后台线程写，主线程读）
type UpdateSlot = Arc<std::sync::Mutex<Option<Result<Option<update::Release>, String>>>>;

/// 在后台线程检查更新，完成后通知主线程
fn check_updates_in_background(
    slot: UpdateSlot,
    tx: async_channel::Sender<AppCommand>,
    manual: bool,
) {
    std::thread::Builder::new()
        .name("tessel-update-check".into())
        .spawn(move || {
            let result = update::check();
            match &result {
                Ok(Some(release)) => log_line(format!("[update] 发现新版本 {}", release.version)),
                Ok(None) => debug_log("[update] 已是最新版本".into()),
                Err(err) => log_line(format!("[update] 检查更新失败：{err}")),
            }
            if let Ok(mut slot) = slot.lock() {
                *slot = Some(result);
            }
            let _ = tx.try_send(AppCommand::UpdateChecked { manual });
        })
        .ok();
}

/// 启动 15 秒后检查一次，之后每 24 小时一次（设置里关掉时跳过）
fn spawn_update_schedule(
    config: Arc<RwLock<Config>>,
    slot: UpdateSlot,
    tx: async_channel::Sender<AppCommand>,
) {
    std::thread::Builder::new()
        .name("tessel-update-schedule".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            loop {
                if config.read().is_ok_and(|c| c.check_updates) {
                    check_updates_in_background(slot.clone(), tx.clone(), false);
                }
                std::thread::sleep(Duration::from_secs(24 * 3600));
            }
        })
        .ok();
}

struct TrayItems {
    /// 托盘菜单本身（有新版本时在顶部插入"下载新版本"）
    menu: Menu,
    /// "下载新版本"：发现新版本后才插入菜单
    update: MenuItem,
    update_shown: bool,
    show: MenuItem,
    pause: CheckMenuItem,
    autostart: CheckMenuItem,
    settings: MenuItem,
    reset: MenuItem,
    clear: MenuItem,
    quit: MenuItem,
}

/// 托盘提示文字
fn tray_tooltip(hotkey_label: &str) -> String {
    trf!(
        "{APP_NAME} {APP_NAME_ZH} · {hotkey_label} 唤起",
        "{APP_NAME} clipboard manager · {hotkey_label} to open"
    )
}

/// 托盘菜单的文字（启动时、切换语言时）
fn set_tray_texts(items: &TrayItems, hotkey_label: &str) {
    items.show.set_text(trf!(
        "显示 Tessel\t{hotkey_label}",
        "Show Tessel\t{hotkey_label}"
    ));
    items.pause.set_text(tr("暂停记录", "Pause recording"));
    items
        .autostart
        .set_text(tr("开机自动启动", "Start with Windows"));
    items.settings.set_text(tr("设置…", "Settings…"));
    items
        .reset
        .set_text(tr("重置窗口位置", "Reset window position"));
    items
        .clear
        .set_text(tr("清空历史（保留收藏）…", "Clear history (keep pinned)…"));
    items.quit.set_text(tr("退出", "Quit"));
}

/// 托盘图标和菜单。对象析构时托盘图标会消失，所以要保存到进程结束。
fn build_tray(
    tx: async_channel::Sender<AppCommand>,
    hotkey_label: &str,
) -> Option<(tray_icon::TrayIcon, TrayItems)> {
    // 文字由 set_tray_texts 统一设置
    let show = MenuItem::new("", true, None);
    let pause = CheckMenuItem::new("", true, false, None);
    let autostart = CheckMenuItem::new("", true, win::autostart_enabled(), None);
    let settings = MenuItem::new("", true, None);
    let reset = MenuItem::new("", true, None);
    let clear = MenuItem::new("", true, None);
    let quit = MenuItem::new("", true, None);
    let update = MenuItem::new("", true, None);
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
        (settings.id().clone(), Some(AppCommand::OpenSettingsWindow)),
        (reset.id().clone(), Some(AppCommand::ResetPosition)),
        (clear.id().clone(), Some(AppCommand::ClearHistory)),
        (quit.id().clone(), Some(AppCommand::Quit)),
        (update.id().clone(), Some(AppCommand::OpenReleasePage)),
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
        .with_tooltip(tray_tooltip(hotkey_label))
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(false)
        .build()
        .ok()?;
    let items = TrayItems {
        menu,
        update,
        update_shown: false,
        show,
        pause,
        autostart,
        settings,
        reset,
        clear,
        quit,
    };
    set_tray_texts(&items, hotkey_label);
    Some((tray, items))
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

/// 全局热键的用途
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HotkeyRole {
    /// 唤起弹窗
    Popup,
    /// 粘贴队列的下一条
    Queue,
}

impl HotkeyRole {
    const ALL: [HotkeyRole; 2] = [HotkeyRole::Popup, HotkeyRole::Queue];

    fn spec(self, config: &Config) -> &str {
        match self {
            HotkeyRole::Popup => &config.hotkey,
            HotkeyRole::Queue => &config.queue_hotkey,
        }
    }

    fn set_spec(self, config: &mut Config, spec: String) {
        match self {
            HotkeyRole::Popup => config.hotkey = spec,
            HotkeyRole::Queue => config.queue_hotkey = spec,
        }
    }

    /// 错误提示的前缀
    fn label(self) -> &'static str {
        match self {
            HotkeyRole::Popup => "",
            HotkeyRole::Queue => tr("粘贴队列的", "Paste queue: "),
        }
    }
}

/// 一个全局热键的注册状态
#[derive(Default)]
struct HotkeySlot {
    /// 当前已注册的热键及其文字形式
    current: Option<(HotKey, String)>,
    /// 给热键回调用的当前热键 id（回调在其他上下文里执行，不能直接访问 `current`）
    active_id: Arc<AtomicU32>,
    /// 录制前的热键，录制结束时新热键注册失败就恢复它
    suspended: Option<(HotKey, String)>,
}

/// 全局热键（唤起弹窗、粘贴队列）：按配置注册，配置变化时换绑
struct Hotkeys {
    manager: GlobalHotKeyManager,
    popup: HotkeySlot,
    queue: HotkeySlot,
    /// 设置窗口正在录制新热键：期间不注册任何热键
    recording: bool,
}

impl Hotkeys {
    fn slot(&self, role: HotkeyRole) -> &HotkeySlot {
        match role {
            HotkeyRole::Popup => &self.popup,
            HotkeyRole::Queue => &self.queue,
        }
    }

    fn slot_mut(&mut self, role: HotkeyRole) -> &mut HotkeySlot {
        match role {
            HotkeyRole::Popup => &mut self.popup,
            HotkeyRole::Queue => &mut self.queue,
        }
    }

    /// 实际生效的热键（录制期间返回录制前的那个）
    fn active_spec(&self, role: HotkeyRole) -> Option<&str> {
        let slot = self.slot(role);
        slot.current
            .as_ref()
            .or(slot.suspended.as_ref())
            .map(|(_, spec)| spec.as_str())
    }

    /// 开始录制：暂时注销所有热键，否则按下它们会被系统截走
    fn suspend(&mut self) {
        if self.recording {
            return;
        }
        self.recording = true;
        for role in HotkeyRole::ALL {
            let slot = self.slot_mut(role);
            slot.suspended = slot.current.take();
            slot.active_id.store(0, Ordering::Relaxed);
            if let Some((hotkey, _)) = self.slot(role).suspended {
                let _ = self.manager.unregister(hotkey);
            }
        }
    }

    /// 结束录制并按 `config` 注册；某个热键失败时恢复它录制前的值，返回失败的热键和说明
    fn resume(&mut self, config: &Config) -> Vec<(HotkeyRole, String)> {
        self.recording = false;
        let mut errors = Vec::new();
        for role in HotkeyRole::ALL {
            let previous = self.slot_mut(role).suspended.take();
            if let Err(err) = self.register(role, role.spec(config)) {
                if let Some((hotkey, old_spec)) = previous
                    && self.manager.register(hotkey).is_ok()
                {
                    self.set_current(role, hotkey, old_spec);
                }
                errors.push((role, err));
            }
        }
        errors
    }

    /// 配置重载、启动时调用，返回注册失败的说明。
    /// 录制期间什么都不做，由录制结束时的 `resume` 统一处理。
    fn apply(&mut self, config: &Config) -> Option<String> {
        if self.recording {
            return None;
        }
        let errors: Vec<String> = HotkeyRole::ALL
            .into_iter()
            .filter_map(|role| {
                let err = self.register(role, role.spec(config)).err()?;
                Some(match self.active_spec(role) {
                    Some(active) => trf!(
                        "{err}，当前仍使用「{active}」",
                        "{err}; still using \"{active}\""
                    ),
                    None => trf!(
                        "{err}，请在设置里换一个",
                        "{err}; please choose another in Settings"
                    ),
                })
            })
            .collect();
        (!errors.is_empty()).then(|| errors.join("；"))
    }

    fn register(&mut self, role: HotkeyRole, spec: &str) -> Result<(), String> {
        // 粘贴队列的热键可以留空（不使用）
        if spec.is_empty() && role == HotkeyRole::Queue {
            let slot = self.slot_mut(role);
            slot.active_id.store(0, Ordering::Relaxed);
            if let Some((old, _)) = slot.current.take() {
                let _ = self.manager.unregister(old);
            }
            return Ok(());
        }
        let prefix = role.label();
        let hotkey = parse_hotkey(spec).map_err(|e| format!("{prefix}{e}"))?;
        if self
            .slot(role)
            .current
            .as_ref()
            .is_some_and(|(h, _)| *h == hotkey)
        {
            return Ok(());
        }
        // 先注册新的，成功后再注销旧的：新热键被占用时旧热键继续可用
        self.manager.register(hotkey).map_err(|_| {
            trf!(
                "{prefix}热键「{spec}」已被占用",
                "{prefix}Hotkey \"{spec}\" is already in use"
            )
        })?;
        if let Some((old, _)) = self.slot_mut(role).current.take() {
            let _ = self.manager.unregister(old);
        }
        self.set_current(role, hotkey, spec.to_string());
        Ok(())
    }

    fn set_current(&mut self, role: HotkeyRole, hotkey: HotKey, spec: String) {
        let slot = self.slot_mut(role);
        slot.active_id.store(hotkey.id(), Ordering::Relaxed);
        slot.current = Some((hotkey, spec));
    }
}

/// 托盘上显示实际生效的热键（而不是配置文件里写的、可能注册失败的那个）
fn update_tray_hotkey(
    tray: Option<&tray_icon::TrayIcon>,
    items: Option<&TrayItems>,
    active: Option<&str>,
) {
    let label = active.unwrap_or(tr("未设置热键", "no hotkey"));
    if let Some(items) = items {
        set_tray_texts(items, label);
    }
    if let Some(tray) = tray {
        let _ = tray.set_tooltip(Some(tray_tooltip(label)));
    }
}

/// GPUI 窗口对应的 Win32 窗口句柄
fn window_hwnd(handle: AnyWindowHandle, cx: &mut AsyncApp) -> Option<isize> {
    handle
        .update(cx, |_, window, _| {
            match window.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
                _ => None,
            }
        })
        .ok()
        .flatten()
}

fn main() {
    // Velopack 安装 / 更新 / 卸载时会带特殊参数启动本程序执行钩子，必须放在最前面
    // （这种情况下它会直接退出进程，不会走到后面的逻辑）。直接运行 exe 时这里什么都不做。
    // 安装程序装好后会直接启动一次程序（first-run）。这时不弹出剪贴板，而是显示安装完成页，
    // 让用户选择要不要现在启动。TESSEL_FIRST_RUN=1 可以强制显示（测试用）。
    let first_run = std::cell::Cell::new(std::env::var_os("TESSEL_FIRST_RUN").is_some());
    velopack::VelopackApp::build()
        // 卸载前去掉开机自启，否则卸载后开机还会去启动一个已不存在的程序。
        // 数据目录（%LOCALAPPDATA%\Tessel）不在安装目录里，卸载不会删除用户的历史。
        .on_before_uninstall_fast_callback(|_| {
            let _ = win::set_autostart(false);
        })
        // 应用更新时 Velopack 会把"设置 → 应用"里的卸载入口改回它自己的，这里再指回我们的卸载程序
        .on_after_update_fast_callback(|_| win::claim_uninstall_entry())
        .on_first_run(|_| first_run.set(true))
        .run();
    let first_run = first_run.get();

    // 单实例：如果已经有一个在运行，通知它显示出来，然后自己退出。
    // 指定了 TESSEL_DATA_DIR 时按数据目录区分实例，测试实例不会和正式实例互相干扰。
    let start_hidden = std::env::args().any(|a| a == "--background");
    let open_settings = std::env::args().any(|a| a == "--settings");
    let Some(signals) = win::acquire_single_instance(open_settings) else {
        return;
    };

    let (tx, rx) = async_channel::unbounded::<AppCommand>();

    let dir = data_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log_line(format!("[error] 无法创建数据目录 {}：{err}", dir.display()));
        return;
    }
    log_line(format!("[store] 数据目录 {}", dir.display()));
    // 更新钩子没跑成功时的兜底（只在卸载入口被改回去时才写注册表）
    win::claim_uninstall_entry();

    // 配置：读取失败时用默认值启动，并在界面上提示
    // 界面语言：读配置之前先按系统语言（配置出错的提示也要能看懂）
    let system_chinese = win::system_is_chinese();
    i18n::set_english(tessel_core::LanguagePref::System.resolve(system_chinese));
    let (config, config_error) = match Config::load_or_create(&dir) {
        Ok(config) => (config, None),
        Err(err) => (
            Config::default(),
            Some(trf!(
                "{err}（已使用默认设置）",
                "{err} (using the default settings)"
            )),
        ),
    };
    i18n::set_english(config.language.resolve(system_chinese));
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

    // 图片文字识别（后台线程，离线）：新采集到图片时唤醒它
    let ocr_wake = ocr::spawn(dir.clone(), tx.clone());

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
                    let notify = !clip.from_self;
                    let is_image = matches!(clip.payload, Payload::Image { .. });
                    let rich = if clip.rich.is_empty() {
                        ""
                    } else {
                        " · 含格式"
                    };
                    let mut store = store.borrow_mut();
                    if let Ok(cfg) = keep_cfg.read() {
                        store.set_keep(cfg.max_items);
                    }
                    match store.record(clip) {
                        Ok(Some(seq)) => {
                            debug_log(format!("[capture] {kind}{rich} · {source} · seq={seq}"));
                            let _ = tx.try_send(AppCommand::ClipboardChanged { notify });
                            if is_image && let Some(wake) = &ocr_wake {
                                let _ = wake.send(());
                            }
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

    // 第二个实例转来的请求：显示弹窗 / 打开设置
    {
        let win::InstanceSignals {
            show,
            settings,
            paste_next,
        } = signals;
        // 命令行 `tessel-cli paste-next`：和队列热键一样
        let paste_tx = tx.clone();
        std::thread::spawn(move || {
            while paste_next.wait() {
                let _ = paste_tx.try_send(AppCommand::PasteNext(Instant::now()));
            }
        });
        let show_tx = tx.clone();
        std::thread::spawn(move || {
            while show.wait() {
                let _ = show_tx.try_send(AppCommand::Show(Instant::now()));
            }
        });
        let settings_tx = tx.clone();
        std::thread::spawn(move || {
            while settings.wait() {
                let _ = settings_tx.try_send(AppCommand::OpenSettingsWindow);
            }
        });
    }

    gpui_kit::application()
        .with_assets(action_menu::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            // 必须在 gpui_kit::init 之后：同深度的键位绑定以后注册的为准
            ui::bind_keys(cx);
            action_menu::bind_keys(cx);

            // 全局热键：global-hotkey 在当前线程创建隐藏窗口接收 WM_HOTKEY，由 GPUI 的消息循环派发
            let mut hotkeys = Hotkeys {
                manager: GlobalHotKeyManager::new().expect("hotkey manager"),
                popup: HotkeySlot::default(),
                queue: HotkeySlot::default(),
                recording: false,
            };
            let mut startup_error = config_error;
            if let Some(err) = hotkeys.apply(&config) {
                log_line(format!("[warn] {err}"));
                startup_error.get_or_insert(err);
            }
            let hotkey_tx = tx.clone();
            let popup_id = hotkeys.popup.active_id.clone();
            let queue_id = hotkeys.queue.active_id.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                debug_log(format!("[hotkey] 回调 state={:?}", event.state));
                if event.state != HotKeyState::Pressed {
                    return;
                }
                if event.id == popup_id.load(Ordering::Relaxed) {
                    let _ = hotkey_tx.try_send(AppCommand::Toggle(Instant::now()));
                } else if event.id == queue_id.load(Ordering::Relaxed) {
                    // 尽快发一个占位键：用户松开 Alt / Shift 时不会激活菜单栏、切换输入法中英文
                    win::send_mask_key();
                    let _ = hotkey_tx.try_send(AppCommand::PasteNext(Instant::now()));
                }
            }));

            // 托盘：tray-icon 同样依赖当前线程的消息循环（GPUI 的）
            let (tray, mut tray_items) = match build_tray(tx.clone(), &config.hotkey) {
                Some((tray, items)) => (Some(tray), Some(items)),
                None => {
                    log_line("[warn] 创建托盘图标失败".into());
                    (None, None)
                }
            };
            update_tray_hotkey(
                tray.as_ref(),
                tray_items.as_ref(),
                hotkeys.active_spec(HotkeyRole::Popup),
            );

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
                        tx.clone(),
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
            // 复制提示（独立的小窗口，不抢焦点）
            if let Some(toast) = toast::open(cx) {
                popup.update(cx, |this, _| this.set_toast(toast));
            }

            let (ws, private) = win::memory_mb();
            log_line(format!(
                "[ready] 按 {} 唤起 · 启动后工作集 {ws:.1}MB · 私有 {private:.1}MB",
                config.hotkey
            ));

            let update_slot: UpdateSlot = Arc::new(std::sync::Mutex::new(None));
            spawn_update_schedule(shared_config.clone(), update_slot.clone(), tx.clone());
            // 最近发现的新版本（托盘"下载新版本"打开它的页面）
            let mut latest_release: Option<update::Release> = None;

            // 双击启动时直接弹出，让用户知道它已经在运行
            if first_run {
                welcome::open(tx.clone(), config.hotkey.clone(), cx);
            } else if open_settings {
                let _ = tx.try_send(AppCommand::OpenSettingsWindow);
            } else if !start_hidden {
                let _ = tx.try_send(AppCommand::Show(Instant::now()));
            }

            let config_path = Config::path(&dir);
            let cmd_tx = tx.clone();
            cx.spawn(async move |cx| {
                // 托盘对象必须活到进程结束
                let tray = tray;
                // 设置窗口：关闭后视图被释放，WeakEntity 升级失败即表示已关闭
                let mut settings_win: Option<SettingsWindow> = None;
                // 设置窗口刚打开时的位置：返回时用来判断用户有没有拖动过它
                let mut settings_opened_at: Option<(i32, i32)> = None;
                while let Ok(cmd) = rx.recv().await {
                    let live_settings = settings_win
                        .as_ref()
                        .and_then(|(h, e)| e.upgrade().map(|e| (*h, e)));
                    match cmd {
                        AppCommand::Quit => {
                            cx.update(|cx| cx.quit());
                            break;
                        }
                        AppCommand::ConfigChanged => {
                            // 文件本身有错才整体拒绝；热键注册失败（被占用）只是警告，其余设置照常生效
                            let result = Config::load_or_create(&dir);
                            let warning = result
                                .as_ref()
                                .ok()
                                .and_then(|new| hotkeys.apply(new));
                            if let Some(w) = &warning {
                                log_line(format!("[config] {w}"));
                            }
                            match &result {
                                Ok(new) => {
                                    log_line(format!("[config] 已重新加载（热键 {}）", new.hotkey));
                                    i18n::set_english(new.language.resolve(system_chinese));
                                    if let Ok(mut shared) = shared_config.write() {
                                        *shared = new.clone();
                                    }
                                }
                                Err(err) => log_line(format!("[config] {err}")),
                            }
                            update_tray_hotkey(
                                tray.as_ref(),
                                tray_items.as_ref(),
                                hotkeys.active_spec(HotkeyRole::Popup),
                            );
                            // 文件有错时整体不生效：说清楚之前的设置仍然有效
                            let result = result.map_err(|err| {
                                trf!("{err}（之前的设置仍然有效）", "{err} (the previous settings stay in effect)")
                            });
                            if let Some((handle, view)) = &live_settings {
                                let result = result.clone();
                                let warning = warning.clone();
                                view.update(cx, |this, cx| {
                                    this.on_config_result(result, warning, cx)
                                });
                                // 主题可能变了，设置窗口也要重画
                                let _ = handle.update(cx, |_, window, _| window.refresh());
                            }
                            let _ = window_handle.update(cx, |_, window, cx| {
                                popup.update(cx, |this, cx| match result {
                                    Ok(new) => {
                                        this.apply_config(new, window, cx);
                                        if let Some(w) = warning {
                                            this.set_config_error(w, cx);
                                        }
                                    }
                                    Err(err) => this.set_config_error(err, cx),
                                });
                            });
                        }
                        AppCommand::OpenSettingsWindow => {
                            if let Some((handle, _)) = &live_settings {
                                let _ = handle.update(cx, |_, window, _| {
                                    if let Ok(RawWindowHandle::Win32(h)) =
                                        window.window_handle().map(|h| h.as_raw())
                                    {
                                        win::restore_and_focus(h.hwnd.get());
                                    }
                                });
                            } else {
                                let config = shared_config
                                    .read()
                                    .map(|c| c.clone())
                                    .unwrap_or_default();
                                let over = popup.read_with(cx, |p, _| p.visible_hwnd());
                                let popup_hwnd = popup.read_with(cx, |p, _| p.hwnd());
                                let opened = cx.update(|cx| {
                                    settings::open(
                                        dir.clone(),
                                        cmd_tx.clone(),
                                        config,
                                        win::autostart_enabled(),
                                        over,
                                        popup_hwnd,
                                        cx,
                                    )
                                });
                                if let Some((_, view)) = &opened {
                                    let known = update_slot.lock().ok().and_then(|s| s.clone());
                                    if let Some(result) = known {
                                        view.update(cx, |v, cx| v.set_update_result(&result, false, cx));
                                    }
                                }
                                if opened.is_some() && over.is_some() {
                                    popup.update(cx, |p, cx| p.yield_to_settings(cx));
                                }
                                settings_opened_at = opened
                                    .as_ref()
                                    .and_then(|(h, _)| window_hwnd(*h, cx))
                                    .and_then(win::window_pos);
                                settings_win = opened.map(|(h, e)| (h, e.downgrade()));
                            }
                        }
                        AppCommand::BackToPopup => {
                            // 弹窗出现在设置窗口现在的位置（看起来是同一个窗口在翻页）；
                            // 用户拖动过设置窗口的话，弹窗以后也记住这个位置
                            let settings_raw = live_settings
                                .as_ref()
                                .and_then(|(handle, _)| window_hwnd(*handle, cx));
                            let dragged = settings_raw.is_some_and(|raw| {
                                win::window_pos(raw).is_some_and(|now| {
                                    settings_opened_at.is_none_or(|at| {
                                        (now.0 - at.0).abs() > 4 || (now.1 - at.1).abs() > 4
                                    })
                                })
                            });
                            // 先把弹窗显示出来（置顶），再关设置窗口，中间不会露出桌面
                            let _ = window_handle.update(cx, |_, window, cx| {
                                popup.update(cx, |this, cx| {
                                    if let Some(raw) = settings_raw {
                                        this.take_position_of(raw, dragged);
                                    }
                                    this.show_if_hidden(Instant::now(), window, cx)
                                })
                            });
                            if let Some((handle, _)) = &live_settings {
                                let _ = handle.update(cx, |_, window, _| window.remove_window());
                            }
                            settings_win = None;
                        }
                        AppCommand::CheckUpdates => {
                            check_updates_in_background(update_slot.clone(), cmd_tx.clone(), true);
                        }
                        AppCommand::UpdateChecked { manual } => {
                            let result = update_slot.lock().ok().and_then(|s| s.clone());
                            let Some(result) = result else { continue };
                            if let Some((_, view)) = &live_settings {
                                view.update(cx, |v, cx| v.set_update_result(&result, manual, cx));
                            }
                            if let Ok(Some(release)) = &result {
                                let first_time = latest_release.as_ref() != Some(release);
                                latest_release = Some(release.clone());
                                if let Some(items) = &mut tray_items {
                                    let version = &release.version;
                                    items.update.set_text(trf!(
                                        "下载新版本 {version}…",
                                        "Download Tessel {version}…"
                                    ));
                                    if !items.update_shown
                                        && items.menu.prepend(&items.update).is_ok()
                                    {
                                        let _ = items.menu.insert(&PredefinedMenuItem::separator(), 1);
                                        items.update_shown = true;
                                    }
                                }
                                // 自动检查发现新版本：提示一次
                                if first_time && !manual {
                                    let version = release.version.clone();
                                    popup.update(cx, |this, cx| this.show_update_notice(&version, cx));
                                }
                            }
                        }
                        AppCommand::OpenReleasePage => {
                            let url = latest_release
                                .as_ref()
                                .map(|r| r.url.clone())
                                .unwrap_or_else(|| update::RELEASES_PAGE.to_string());
                            cx.update(|cx| cx.open_url(&url));
                        }
                        AppCommand::OpenConfigFile => {
                            // 记事本一定存在；.toml 不一定关联了编辑器
                            if let Err(err) = std::process::Command::new("notepad.exe")
                                .arg(&config_path)
                                .spawn()
                            {
                                log_line(format!("[error] 无法打开设置文件：{err}"));
                            }
                        }
                        AppCommand::AutostartToggled | AppCommand::SetAutostart(_) => {
                            let want = match cmd {
                                AppCommand::SetAutostart(want) => want,
                                _ => tray_items
                                    .as_ref()
                                    .is_some_and(|t| t.autostart.is_checked()),
                            };
                            if let Err(err) = win::set_autostart(want) {
                                log_line(format!("[error] 设置开机自启失败：{err}"));
                            }
                            // 以注册表的实际状态为准，同步托盘和设置窗口
                            let actual = win::autostart_enabled();
                            if let Some(items) = &tray_items {
                                items.autostart.set_checked(actual);
                            }
                            if let Some((_, view)) = &live_settings {
                                view.update(cx, |this, cx| this.set_autostart_state(actual, cx));
                            }
                        }
                        AppCommand::FinishSetup { launch, autostart } => {
                            if let Err(err) = win::set_autostart(autostart) {
                                log_line(format!("[error] 设置开机自启失败：{err}"));
                            }
                            if let Some(items) = &tray_items {
                                items.autostart.set_checked(win::autostart_enabled());
                            }
                            if launch {
                                let _ = window_handle.update(cx, |_, window, cx| {
                                    popup.update(cx, |this, cx| {
                                        this.show_if_hidden(Instant::now(), window, cx)
                                    })
                                });
                            } else {
                                // 用户选择先不启动：直接退出，不在后台运行
                                log_line("[setup] 用户选择稍后启动，退出".into());
                                cx.update(|cx| cx.quit());
                                break;
                            }
                        }
                        AppCommand::HotkeyRecording(true) => hotkeys.suspend(),
                        AppCommand::HotkeyRecording(false) => {
                            // 录制期间收到的 ConfigChanged 已经把新热键写进 shared_config
                            let config = shared_config
                                .read()
                                .map(|c| c.clone())
                                .unwrap_or_default();
                            let errors = hotkeys.resume(&config);
                            // 新热键没注册上：把配置文件改回实际生效的热键，避免下次启动时没有热键
                            if !errors.is_empty()
                                && let Ok(mut file) = Config::load_or_create(&dir)
                            {
                                let mut notices = Vec::new();
                                for (role, err) in &errors {
                                    log_line(format!("[warn] {err}"));
                                    let active = hotkeys
                                        .active_spec(*role)
                                        .unwrap_or_default()
                                        .to_string();
                                    notices.push(if active.is_empty() {
                                        err.clone()
                                    } else {
                                        trf!(
                                            "{err}，已恢复为「{active}」",
                                            "{err}; restored \"{active}\""
                                        )
                                    });
                                    // 唤起热键没有可恢复的旧值时保留配置里的写法，下次启动再试
                                    if !active.is_empty() || *role == HotkeyRole::Queue {
                                        role.set_spec(&mut file, active);
                                    }
                                }
                                if file.save(&dir).is_ok()
                                    && let Ok(mut shared) = shared_config.write()
                                {
                                    *shared = file.clone();
                                }
                                if let Some((_, view)) = &live_settings {
                                    let notice = notices.join("；");
                                    view.update(cx, |this, cx| {
                                        this.on_config_result(Ok(file), None, cx);
                                        this.set_notice(notice, cx);
                                    });
                                }
                            }
                            update_tray_hotkey(
                                tray.as_ref(),
                                tray_items.as_ref(),
                                hotkeys.active_spec(HotkeyRole::Popup),
                            );
                        }
                        AppCommand::PauseToggled => {
                            if let Some(items) = &tray_items {
                                let want = items.pause.is_checked();
                                popup.update(cx, |this, cx| this.set_paused(want, cx));
                            }
                        }
                        AppCommand::ClearHistory => {
                            if win::confirm(
                                tr("清空历史 · Tessel", "Clear history · Tessel"),
                                tr(
                                    "确定要清空所有未收藏的剪贴板历史吗？\n收藏的内容和片段会保留。此操作无法撤销。",
                                    "Delete all clipboard history that isn't pinned?\nPinned items and snippets are kept. This can't be undone.",
                                ),
                            ) {
                                popup.update(cx, |this, cx| this.clear_history(cx));
                                if let Some((_, view)) = &live_settings {
                                    view.update(cx, |this, cx| this.refresh_storage(cx));
                                }
                            }
                        }
                        cmd => {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                popup.update(cx, |this, cx| match cmd {
                                    AppCommand::Toggle(t0) => this.toggle(t0, window, cx),
                                    AppCommand::Show(t0) => this.show_if_hidden(t0, window, cx),
                                    AppCommand::ClipboardChanged { notify } => {
                                        this.on_clipboard_changed(notify, cx)
                                    }
                                    AppCommand::PasteNext(t0) => this.paste_next(t0, cx),
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
