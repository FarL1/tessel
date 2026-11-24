//! 弹出面板的界面：搜索栏、分类标签、列表、预览、快捷键提示。

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_clipboard::WriteContent;
use tessel_core::time::{now_ms, relative as relative_time};
use tessel_core::{Clip, Config, Counts, Filter, Kind, Query, Store, ThemePref};

use crate::action_menu::{self, ActionMenu, MenuAction};
use crate::design::{self, Palette, parse_hex_color};
use crate::toast::{self, Notice, Toast, Tone};
use crate::{AppCommand, debug_log, log_line, win};

actions!(
    clip_popup,
    [
        SelectPrev,
        SelectNext,
        NextFilter,
        PrevFilter,
        Dismiss,
        PasteSelected,
        PastePlainText,
        TogglePin,
        DeleteClip,
        OpenClip,
        OpenSettings,
        OpenActionMenu,
        ToggleQueue,
        StartEdit,
        SaveEdit,
        SaveEditAndPaste,
        CancelEdit
    ]
);

const CONTEXT: &str = "ClipPopup";
/// 编辑记录时预览区变成编辑框，键位上下文是 `ClipPopup > ClipEditor`
const EDITOR_CONTEXT: &str = "ClipEditor";
/// 一次最多加载的条数。列表是虚拟列表（只渲染可见的行），条数主要影响内存：
/// 列表查询的内容已截断为 4000 字符，预览和粘贴时再按需读取完整内容。
const RESULT_LIMIT: usize = 1000;
/// 预览区最多显示的字符数，避免超长文本拖慢渲染
const PREVIEW_CHARS: usize = 8_000;

/// 弹窗的键位表。正式版本里会从用户配置文件加载，允许自定义。
///
/// 谓词写成 `ClipPopup > Input`：焦点在搜索框里时，搜索框自身也绑定了 up/down/enter/tab，
/// 同一深度的绑定以后注册的为准，所以这些绑定必须在 `gpui_kit::init` 之后注册。
pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for ctx in ["ClipPopup > Input", CONTEXT] {
        let c = Some(ctx);
        bindings.extend([
            KeyBinding::new("up", SelectPrev, c),
            KeyBinding::new("down", SelectNext, c),
            KeyBinding::new("tab", NextFilter, c),
            KeyBinding::new("shift-tab", PrevFilter, c),
            KeyBinding::new("escape", Dismiss, c),
            KeyBinding::new("enter", PasteSelected, c),
            KeyBinding::new("shift-enter", PastePlainText, c),
            KeyBinding::new("ctrl-p", TogglePin, c),
            KeyBinding::new("shift-delete", DeleteClip, c),
            KeyBinding::new("ctrl-o", OpenClip, c),
            KeyBinding::new("ctrl-,", OpenSettings, c),
            KeyBinding::new("ctrl-k", OpenActionMenu, c),
            KeyBinding::new("ctrl-q", ToggleQueue, c),
            KeyBinding::new("ctrl-e", StartEdit, c),
        ]);
    }
    // 编辑框里：Enter 是换行，Esc 只退出编辑（不关闭弹窗）
    for ctx in ["ClipEditor > Input", EDITOR_CONTEXT] {
        let c = Some(ctx);
        bindings.extend([
            KeyBinding::new("ctrl-s", SaveEdit, c),
            KeyBinding::new("ctrl-enter", SaveEditAndPaste, c),
            KeyBinding::new("escape", CancelEdit, c),
        ]);
    }
    cx.bind_keys(bindings);
}

pub struct Popup {
    hwnd: isize,
    /// 给主线程发命令（打开设置窗口等）
    tx: async_channel::Sender<AppCommand>,
    visible: bool,
    /// 弹出前的前台窗口，粘贴时切回它
    prev_foreground: Option<isize>,
    store: Store,
    paused: Arc<AtomicBool>,
    /// 暂停状态在界面里切换时通知托盘
    on_pause_changed: Box<dyn Fn(bool)>,
    config: Config,
    search: Entity<InputState>,
    query: String,
    /// 结果高亮用的关键词（去掉了 app: type: 之类的前缀过滤）
    highlight: String,
    filter_ix: usize,
    items: Vec<Clip>,
    /// 当前选中项的完整内容（列表里的内容是截断过的）
    preview: Option<Clip>,
    counts: Counts,
    /// 在当前结果中的位置
    selected: usize,
    /// 渲染时用的"现在"，用来算"5 分钟前"
    now: i64,
    show_count: usize,
    scroll: UniformListScrollHandle,
    status: Option<SharedString>,
    /// 配置文件的错误：一直显示在底栏，直到改正
    config_error: Option<SharedString>,
    logo: Arc<Image>,
    /// 上次自动摆放的位置；隐藏时如果窗口不在这里，说明用户拖动过
    placed_at: Option<(i32, i32)>,
    /// 用户拖动后的位置，下次唤起时沿用
    user_pos: Option<(i32, i32)>,
    /// 打开中的动作菜单（Ctrl+K）
    menu: Option<ActionMenu>,
    menu_search: Entity<InputState>,
    /// 复制提示、粘贴队列进度
    toast: Option<Entity<Toast>>,
    /// 粘贴队列（记录的 uuid），每按一次队列热键粘贴最前面的一条
    queue: VecDeque<String>,
    /// 还没处理的队列热键次数（等用户松开修饰键期间又按了几次）
    queue_requests: usize,
    queue_busy: bool,
    /// 正在编辑的记录
    editing: Option<String>,
    editor: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl Popup {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hwnd: isize,
        tx: async_channel::Sender<AppCommand>,
        store: Store,
        paused: Arc<AtomicBool>,
        config: Config,
        on_pause_changed: Box<dyn Fn(bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("搜索剪贴板历史…   支持 app:微信  type:链接  -排除")
        });
        let menu_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索动作…"));
        let editor = cx.new(|cx| TextareaState::new(window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&menu_search, window, |this, state, event, _window, cx| {
                if let InputEvent::Change = event
                    && let Some(menu) = &mut this.menu
                {
                    menu.set_query(&state.read(cx).value());
                    cx.notify();
                }
            }),
            cx.subscribe_in(&search, window, |this, state, event, _window, cx| {
                if let InputEvent::Change = event {
                    this.query = state.read(cx).value().to_string();
                    this.highlight = Query::parse(&this.query).highlight();
                    this.selected = 0;
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    this.refresh(cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                debug_log(format!(
                    "[event] 激活状态 active={active} visible={}",
                    this.visible
                ));
                if this.visible && !active {
                    // 点击外部时自动隐藏
                    this.hide(false, cx);
                } else if this.visible && active {
                    // GPUI 处理激活时会重置内部焦点，激活事件可能晚于 show() 到达，这里重新聚焦
                    if this.editing.is_some() {
                        this.editor.update(cx, |state, cx| state.focus(window, cx));
                    } else {
                        let input = if this.menu.is_some() {
                            &this.menu_search
                        } else {
                            &this.search
                        };
                        input.update(cx, |state, cx| state.focus(window, cx));
                    }
                }
            }),
            // 主题设为"跟随系统"时，系统切换深浅色后实时更新
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.config.theme == ThemePref::System {
                    Theme::sync_system_appearance(Some(window), cx);
                    design::apply_brand(cx);
                }
            }),
        ];

        let mut this = Self {
            hwnd,
            tx,
            visible: false,
            prev_foreground: None,
            store,
            paused,
            on_pause_changed,
            config,
            search,
            query: String::new(),
            highlight: String::new(),
            filter_ix: 0,
            items: Vec::new(),
            preview: None,
            counts: Counts::default(),
            selected: 0,
            now: now_ms(),
            show_count: 0,
            scroll: UniformListScrollHandle::new(),
            status: None,
            config_error: None,
            logo: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../../../assets/brand/png/tessel-64.png").to_vec(),
            )),
            placed_at: None,
            user_pos: None,
            menu: None,
            menu_search,
            toast: None,
            queue: VecDeque::new(),
            queue_requests: 0,
            queue_busy: false,
            editing: None,
            editor,
            _subscriptions: subscriptions,
        };
        this.apply_theme(window, cx);
        this.refresh(cx);
        this
    }

    fn apply_theme(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.config.theme {
            ThemePref::System => Theme::sync_system_appearance(Some(window), cx),
            ThemePref::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            ThemePref::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        }
        design::apply_brand(cx);
    }

    /// 配置文件重新加载后调用
    pub fn apply_config(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
        self.config = config;
        self.config_error = None;
        self.apply_theme(window, cx);
        cx.notify();
    }

    pub fn set_config_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.config_error = Some(error.into());
        cx.notify();
    }

    /// 在底部状态栏显示一条提示（下次唤起时清除）
    pub fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.status = Some(status.into());
        cx.notify();
    }

    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused.store(paused, Ordering::Relaxed);
        (self.on_pause_changed)(paused);
        self.set_status(
            if paused {
                "已暂停记录"
            } else {
                "已恢复记录"
            }
            .into(),
            cx,
        );
    }

    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        match self.store.clear_unpinned() {
            Ok(n) => self.status = Some(format!("已清空 {n} 条历史").into()),
            Err(err) => self.status = Some(format!("清空失败：{err}").into()),
        }
        self.selected = 0;
        self.refresh(cx);
    }

    /// 读取第 `pos` 条的完整内容（列表里的是截断过的）
    fn full_clip(&self, pos: usize) -> Option<Clip> {
        let listed = self.items.get(pos)?;
        match self.store.get(&listed.uuid) {
            Ok(Some(clip)) => Some(clip),
            _ => Some(listed.clone()),
        }
    }

    fn filter(&self) -> Filter {
        Filter::ALL[self.filter_ix]
    }

    /// 按当前搜索词和分类重新查询
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        match self.store.search(&self.query, self.filter(), RESULT_LIMIT) {
            Ok(items) => {
                self.items = items;
                // 记录可能被重新采集过（uuid 不变，但使用次数、时间、富格式会变）
                self.preview = None;
            }
            Err(err) => log_line(format!("[store] 搜索失败：{err}")),
        }
        if let Ok(counts) = self.store.counts() {
            self.counts = counts;
        }
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
        self.now = now_ms();
        debug_log(format!(
            "[store] 查询「{}」{} 条 · {:.2}ms",
            self.query,
            self.items.len(),
            t0.elapsed().as_secs_f64() * 1000.0
        ));
        cx.notify();
    }

    pub fn set_toast(&mut self, toast: Entity<Toast>) {
        self.toast = Some(toast);
    }

    fn show_toast(&self, notice: Notice, cx: &mut Context<Self>) {
        if let Some(toast) = &self.toast {
            toast.update(cx, |t, cx| t.show(notice, cx));
        }
    }

    /// 采集线程写入了新内容。`notify`：用户自己复制的（不是 Tessel 写回的），提示"已复制"
    pub fn on_clipboard_changed(&mut self, notify: bool, cx: &mut Context<Self>) {
        if self.visible {
            self.refresh(cx);
            return;
        }
        if notify
            && self.config.copy_toast
            && let Ok(latest) = self.store.search("", Filter::All, 1)
            && let Some(clip) = latest.first()
        {
            self.show_toast(Notice::copied(clip), cx);
        }
    }

    pub fn reset_position(&mut self) {
        self.user_pos = None;
    }

    /// 托盘点击、再次启动：只负责显示，已经显示时保持不动
    /// 已经显示但没有被激活（唤起方没有前台权限时 SetForegroundWindow 会失败）也重新显示，
    /// 否则弹窗既拿不到键盘，也收不到"失去激活"事件，会一直挂在那里
    pub fn show_if_hidden(&mut self, t0: Instant, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible || !window.is_window_active() {
            self.show(t0, window, cx);
        }
    }

    pub fn toggle(&mut self, t0: Instant, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.hide(true, cx);
        } else {
            self.show(t0, window, cx);
        }
    }

    fn show(&mut self, t0: Instant, window: &mut Window, cx: &mut Context<Self>) {
        let prev = win::foreground();
        self.prev_foreground = (prev != self.hwnd && prev != 0).then_some(prev);
        self.visible = true;
        self.menu = None;
        self.editing = None;
        self.selected = 0;
        self.filter_ix = 0;
        self.status = None;
        self.show_count += 1;
        self.query.clear();
        self.highlight.clear();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.refresh(cx);

        // 用户拖动过就沿用那个位置（如果还在某块屏幕上），否则放在鼠标所在屏幕的偏上位置
        match self.user_pos.filter(|&p| win::is_on_screen(p)) {
            Some(pos) => win::move_to(self.hwnd, pos),
            None => {
                self.user_pos = None;
                win::center_on_cursor_monitor(self.hwnd);
            }
        }
        self.placed_at = win::window_pos(self.hwnd);

        // 注意：不要调用 window.activate_window()。GPUI 在 Windows 上的实现是异步的，
        // 会重新应用创建时的窗口位置状态，还会注入一次 Alt 键。收到 WM_HOTKEY 时我们本身就有前台权限。
        if !win::show_and_focus(self.hwnd) {
            log_line("[show] ⚠ SetFocus 之后键盘焦点仍不在弹窗上".into());
        }
        self.search.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });

        let shown = t0.elapsed();
        let view = cx.entity().downgrade();
        window.on_next_frame(move |_window, cx| {
            let frame = t0.elapsed();
            let _ = view.update(cx, |_, _| {
                log_line(format!(
                    "[show] 热键→ShowWindow {:.1}ms · 热键→首帧 {:.1}ms",
                    shown.as_secs_f64() * 1000.0,
                    frame.as_secs_f64() * 1000.0,
                ));
            });
        });
        cx.notify();
    }

    fn remember_position(&mut self) {
        let current = win::window_pos(self.hwnd);
        if current.is_some() && current != self.placed_at {
            self.user_pos = current;
        }
    }

    /// `restore_focus`：Esc / 热键关闭时把焦点还给之前的窗口；点击外部时不需要
    fn hide(&mut self, restore_focus: bool, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.remember_position();
        self.visible = false;
        self.menu = None;
        // 没保存的修改直接丢弃（点到外面、按热键关闭时）
        self.editing = None;
        win::hide(self.hwnd);
        if restore_focus && let Some(prev) = self.prev_foreground {
            win::focus(prev);
        }
        let (ws, private) = win::memory_mb();
        debug_log(format!("[hide] 工作集 {ws:.1}MB · 私有 {private:.1}MB"));
        cx.notify();
    }

    /// `plain`：只写纯文本，丢掉 HTML / RTF 格式
    fn write_to_clipboard(&self, clip: &Clip, plain: bool) -> Result<(), String> {
        let owner = Some(self.hwnd);
        match clip.kind {
            Kind::File => {
                tessel_clipboard::write(owner, WriteContent::Files(clip.files().collect()))
            }
            Kind::Image => {
                let path = &clip.image.as_ref().ok_or("图片文件丢失")?.path;
                let png = std::fs::read(path).map_err(|e| format!("读取图片失败：{e}"))?;
                tessel_clipboard::write(owner, WriteContent::ImagePng(&png))
            }
            _ if clip.has_rich && !plain => {
                let rich = self
                    .store
                    .rich_formats(&clip.uuid)
                    .map_err(|e| format!("读取格式失败：{e}"))?;
                tessel_clipboard::write(
                    owner,
                    WriteContent::RichText {
                        text: &clip.content,
                        html: rich.html.as_deref(),
                        rtf: rich.rtf.as_deref(),
                    },
                )
            }
            _ => tessel_clipboard::write(owner, WriteContent::Text(&clip.content)),
        }
    }

    fn paste(&mut self, pos: usize, plain: bool, cx: &mut Context<Self>) {
        if let Some(clip) = self.full_clip(pos) {
            self.paste_clip(&clip, plain, cx);
        }
    }

    fn paste_clip(&mut self, clip: &Clip, plain: bool, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        let written = self.write_to_clipboard(clip, plain);
        let what = format!(
            "{}{}",
            clip.kind.label(),
            if plain { "（纯文本）" } else { "" }
        );
        self.deliver(written, &what, t0, cx);
    }

    /// 粘贴一段生成的文本（转换结果、文件路径）。Windows 剪贴板文本惯用 CRLF。
    fn paste_text(&mut self, text: &str, what: &str, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
        let written = tessel_clipboard::write(Some(self.hwnd), WriteContent::Text(&text));
        self.deliver(written, what, t0, cx);
    }

    /// 只复制到剪贴板，关闭弹窗，由用户自己粘贴
    fn copy_only(&mut self, pos: usize, cx: &mut Context<Self>) {
        let Some(clip) = self.full_clip(pos) else {
            return;
        };
        match self.write_to_clipboard(&clip, false) {
            Ok(()) => self.hide(true, cx),
            Err(err) => {
                self.status = Some(format!("写入剪贴板失败：{err}").into());
                cx.notify();
            }
        }
    }

    /// 已经写好剪贴板之后：切回原窗口并粘贴（或者按设置只复制）
    fn deliver(
        &mut self,
        written: Result<(), String>,
        what: &str,
        t0: Instant,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = written {
            self.status = Some(format!("写入剪贴板失败：{err}").into());
            cx.notify();
            return;
        }
        // 设置为"只复制"时：关闭弹窗、把焦点还回去，由用户自己粘贴
        if !self.config.auto_paste {
            self.hide(true, cx);
            return;
        }
        let Some(target) = self.prev_foreground.take() else {
            self.status = Some("已复制到剪贴板".into());
            cx.notify();
            return;
        };
        self.remember_position();
        self.visible = false;
        win::hide(self.hwnd);
        let report = win::restore_and_paste(target);
        log_line(format!(
            "[paste] {} {} → 「{}」· 切换焦点 {:.1}ms · 总耗时 {:.1}ms",
            if report.focused {
                "✅"
            } else {
                "❌ 无法切回目标窗口"
            },
            what,
            report.target_title,
            report.focus_ms,
            t0.elapsed().as_secs_f64() * 1000.0,
        ));
        cx.notify();
    }

    // ── 粘贴队列 ──

    /// 队列热键的文字（提示用）
    fn queue_hotkey_hint(&self) -> String {
        if self.config.queue_hotkey.is_empty() {
            "先在设置里设定粘贴队列热键".into()
        } else {
            format!("按 {} 依次粘贴", self.config.queue_hotkey)
        }
    }

    /// 把第 `pos` 条加入 / 移出粘贴队列
    fn toggle_queue(&mut self, pos: usize, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(pos) else {
            return;
        };
        let status = if let Some(i) = self.queue.iter().position(|u| *u == clip.uuid) {
            self.queue.remove(i);
            if self.queue.is_empty() {
                "已移出，粘贴队列空了".to_string()
            } else {
                format!("已移出粘贴队列 · 还有 {} 条", self.queue.len())
            }
        } else {
            self.queue.push_back(clip.uuid.clone());
            format!(
                "已加入粘贴队列第 {} 条 · {}",
                self.queue.len(),
                self.queue_hotkey_hint()
            )
        };
        self.status = Some(status.into());
        cx.notify();
    }

    fn clear_queue(&mut self, cx: &mut Context<Self>) {
        self.queue.clear();
        self.status = Some("已清空粘贴队列".into());
        cx.notify();
    }

    /// 队列热键：粘贴队列里的下一条到当前窗口。
    /// 热键触发时用户还按着修饰键（比如 Alt+Shift），等松开之后再发 Ctrl+V，
    /// 否则目标程序收到的是 Ctrl+Alt+Shift+V。
    pub fn paste_next(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            self.hide(true, cx);
        }
        self.queue_requests += 1;
        if self.queue_busy {
            return;
        }
        self.queue_busy = true;
        cx.spawn(async move |this, cx| {
            loop {
                let t0 = Instant::now();
                while win::modifiers_held() && t0.elapsed() < Duration::from_secs(3) {
                    cx.background_executor()
                        .timer(Duration::from_millis(15))
                        .await;
                }
                let again = this
                    .update(cx, |this, cx| {
                        if win::modifiers_held() {
                            this.queue_requests = 0;
                            this.show_toast(
                                Notice::new(Tone::Warning, Lucide::ListOrdered, "没有粘贴")
                                    .detail("松开热键之后才会粘贴，请再按一次"),
                                cx,
                            );
                        } else {
                            this.queue_requests = this.queue_requests.saturating_sub(1);
                            this.paste_queue_head(cx);
                        }
                        let again = this.queue_requests > 0;
                        if !again {
                            this.queue_busy = false;
                        }
                        again
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
                // 连续粘贴之间留一点时间，让目标程序先读完剪贴板
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
            }
        })
        .detach();
    }

    fn paste_queue_head(&mut self, cx: &mut Context<Self>) {
        while let Some(uuid) = self.queue.pop_front() {
            // 排队之后被删掉的记录直接跳过
            let Ok(Some(clip)) = self.store.get(&uuid) else {
                continue;
            };
            let notice = match self.write_to_clipboard(&clip, false) {
                Ok(()) => {
                    win::send_ctrl_v();
                    log_line(format!(
                        "[queue] 粘贴{} → 「{}」· 还剩 {} 条",
                        clip.kind.label(),
                        win::window_title(win::foreground()),
                        self.queue.len()
                    ));
                    let left = self.queue.len();
                    Notice::new(
                        if left == 0 { Tone::Success } else { Tone::Info },
                        Lucide::ListOrdered,
                        if left == 0 {
                            "队列已全部粘贴"
                        } else {
                            "已粘贴"
                        },
                    )
                    .tag(if left == 0 {
                        "最后一条".to_string()
                    } else {
                        format!("还剩 {left} 条")
                    })
                    .detail(toast::summary(&clip))
                }
                Err(err) => Notice::new(Tone::Warning, Lucide::ListOrdered, "粘贴失败")
                    .detail(format!("写入剪贴板失败：{err}")),
            };
            self.show_toast(notice, cx);
            cx.notify();
            return;
        }
        self.show_toast(
            Notice::new(Tone::Warning, Lucide::ListOrdered, "粘贴队列是空的")
                .detail("在 Tessel 里选中记录，按 Ctrl+Q 加入队列"),
            cx,
        );
    }

    // ── 编辑 ──

    fn start_edit(&mut self, pos: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.full_clip(pos) else {
            return;
        };
        if matches!(clip.kind, Kind::Image | Kind::File) {
            self.status = Some("图片和文件不能编辑".into());
            cx.notify();
            return;
        }
        self.menu = None;
        self.selected = pos;
        self.editing = Some(clip.uuid.clone());
        self.editor.update(cx, |state, cx| {
            state.set_value(clip.content.replace("\r\n", "\n"), window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.search.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// 保存修改；`then_paste`：保存后直接粘贴
    fn save_edit(&mut self, then_paste: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(uuid) = self.editing.clone() else {
            return;
        };
        let Ok(Some(original)) = self.store.get(&uuid) else {
            self.status = Some("这条记录已经不存在了".into());
            self.cancel_edit(window, cx);
            return;
        };
        let mut text = self.editor.read(cx).value().to_string();
        if text.trim().is_empty() {
            self.status = Some("内容不能为空（要删除这条记录，请按 Esc 后再按 Shift+Del）".into());
            cx.notify();
            return;
        }
        // 编辑框里统一用 \n；原文是 Windows 换行就还原回去
        if original.content.contains("\r\n") {
            text = text.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        if text != original.content {
            match self.store.update_text(&uuid, &text) {
                Ok(Some(_)) => self.status = Some("已保存修改".into()),
                Ok(None) => {
                    self.status = Some("这条记录不能修改".into());
                    cx.notify();
                    return;
                }
                Err(err) => {
                    self.status = Some(format!("保存失败：{err}").into());
                    cx.notify();
                    return;
                }
            }
        }
        self.editing = None;
        self.refresh(cx);
        if let Some(pos) = self.items.iter().position(|c| c.uuid == uuid) {
            self.selected = pos;
            self.scroll.scroll_to_item(pos, ScrollStrategy::Nearest);
        }
        self.search.update(cx, |state, cx| state.focus(window, cx));
        if then_paste && let Ok(Some(clip)) = self.store.get(&uuid) {
            self.paste_clip(&clip, false, cx);
        }
    }

    fn on_toggle_queue(&mut self, _: &ToggleQueue, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            self.toggle_queue(self.selected, cx);
        }
    }

    fn on_start_edit(&mut self, _: &StartEdit, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            self.start_edit(self.selected, window, cx);
        }
    }

    fn on_save_edit(&mut self, _: &SaveEdit, window: &mut Window, cx: &mut Context<Self>) {
        self.save_edit(false, window, cx);
    }

    fn on_save_edit_and_paste(
        &mut self,
        _: &SaveEditAndPaste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_edit(true, window, cx);
    }

    fn on_cancel_edit(&mut self, _: &CancelEdit, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_edit(window, cx);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_filter(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.filter_ix = ix % Filter::ALL.len();
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.refresh(cx);
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let paused = !self.paused.load(Ordering::Relaxed);
        self.set_paused(paused, cx);
    }

    /// Ctrl+1–9 快速粘贴。数字键没有被其他组件绑定，用捕获阶段的按键监听即可。
    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        debug_log(format!("[event] key_down {key:?} {mods:?}"));
        if self.menu.is_none()
            && self.editing.is_none()
            && mods.control
            && key.len() == 1
            && (b'1'..=b'9').contains(&key.as_bytes()[0])
        {
            self.paste((key.as_bytes()[0] - b'1') as usize, mods.shift, cx);
            cx.stop_propagation();
        }
    }

    fn on_select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn on_next_filter(&mut self, _: &NextFilter, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.select_filter(self.filter_ix + 1, cx);
    }

    fn on_prev_filter(&mut self, _: &PrevFilter, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.select_filter(self.filter_ix + Filter::ALL.len() - 1, cx);
    }

    fn on_dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.hide(true, cx);
    }

    fn on_paste(&mut self, _: &PasteSelected, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.paste(self.selected, false, cx);
    }

    fn on_paste_plain(&mut self, _: &PastePlainText, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.paste(self.selected, true, cx);
    }

    fn on_toggle_pin(&mut self, _: &TogglePin, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.toggle_pin(self.selected, cx);
    }

    fn toggle_pin(&mut self, pos: usize, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(pos) else {
            return;
        };
        let pinned = !clip.pinned;
        if let Err(err) = self.store.set_pinned(&clip.uuid, pinned) {
            self.status = Some(format!("操作失败：{err}").into());
        } else {
            self.status = Some(
                if pinned {
                    "已收藏"
                } else {
                    "已取消收藏"
                }
                .into(),
            );
        }
        self.refresh(cx);
    }

    fn on_delete(&mut self, _: &DeleteClip, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.delete(self.selected, cx);
    }

    fn delete(&mut self, pos: usize, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(pos) else {
            return;
        };
        if let Err(err) = self.store.delete(&clip.uuid) {
            self.status = Some(format!("删除失败：{err}").into());
        } else {
            self.status = Some("已删除".into());
        }
        self.refresh(cx);
    }

    fn on_open_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(cx);
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        // 先隐藏弹窗再打开设置窗口（弹窗是置顶的，不收起会挡住设置窗口）
        self.hide(false, cx);
        let _ = self.tx.try_send(AppCommand::OpenSettingsWindow);
    }

    fn on_open_menu(&mut self, _: &OpenActionMenu, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let Some(clip) = self.full_clip(self.selected) else {
            return;
        };
        self.menu_search
            .update(cx, |state, cx| state.set_value("", window, cx));
        let queued = self.queue.iter().position(|u| *u == clip.uuid);
        self.menu = Some(ActionMenu::new(&clip, queued, self.queue.len()));
        self.menu_search
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.search.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn on_menu_prev(&mut self, _: &action_menu::MenuPrev, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.move_selection(-1);
            cx.notify();
        }
    }

    fn on_menu_next(&mut self, _: &action_menu::MenuNext, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.move_selection(1);
            cx.notify();
        }
    }

    fn on_menu_close(
        &mut self,
        _: &action_menu::MenuClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(window, cx);
    }

    fn on_menu_confirm(
        &mut self,
        _: &action_menu::MenuConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_entry(window, cx);
    }

    /// 鼠标点了菜单里第 `pos` 个可见条目
    fn pick_menu_entry(&mut self, pos: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.select(pos);
        }
        self.run_menu_entry(window, cx);
    }

    fn run_menu_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let Some(entry) = menu.current() else {
            self.menu = Some(menu);
            return;
        };
        let (action, output) = (entry.action, entry.output.clone());
        self.close_menu(window, cx);
        // 菜单打开期间列表可能刷新过（采集到新内容），按 uuid 重新定位
        let Some(pos) = self.items.iter().position(|c| c.uuid == menu.uuid) else {
            self.status = Some("这条记录已经不存在了".into());
            return;
        };
        self.selected = pos;
        match action {
            MenuAction::Paste => self.paste(pos, false, cx),
            MenuAction::PastePlain => self.paste(pos, true, cx),
            MenuAction::CopyOnly => self.copy_only(pos, cx),
            MenuAction::PastePaths => {
                if let Some(clip) = self.full_clip(pos) {
                    let paths = clip.files().collect::<Vec<_>>().join("\n");
                    self.paste_text(&paths, "文件路径", cx);
                }
            }
            MenuAction::Open => self.open(pos, cx),
            MenuAction::Transform(t) => {
                if let Some(output) = output {
                    self.paste_text(&output, t.label(), cx);
                }
            }
            MenuAction::TogglePin => self.toggle_pin(pos, cx),
            MenuAction::Delete => self.delete(pos, cx),
            MenuAction::Edit => self.start_edit(pos, window, cx),
            MenuAction::ToggleQueue => self.toggle_queue(pos, cx),
            MenuAction::ClearQueue => self.clear_queue(cx),
        }
    }

    fn on_open(&mut self, _: &OpenClip, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.open(self.selected, cx);
    }

    fn open(&mut self, pos: usize, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(pos) else {
            return;
        };
        match clip.kind {
            Kind::Link => {
                let url = clip.content.trim();
                let url = if url.starts_with("www.") {
                    format!("https://{url}")
                } else {
                    url.to_string()
                };
                cx.open_url(&url);
            }
            Kind::File => {
                if let Some(first) = clip.files().next() {
                    cx.reveal_path(std::path::Path::new(first));
                }
            }
            Kind::Image => {
                if let Some(img) = &clip.image {
                    cx.open_with_system(&img.path);
                }
            }
            _ => {}
        }
    }
}

// ───────────────────────────── 渲染 ─────────────────────────────

fn kind_icon(kind: Kind) -> IconName {
    match kind {
        Kind::Text => IconName::FileText,
        Kind::Link => IconName::Globe,
        Kind::Code => IconName::SquareTerminal,
        Kind::Color => IconName::Palette,
        Kind::Image => IconName::Frame,
        Kind::File => IconName::Folder,
    }
}

/// 列表左侧的类型徽标：颜色显示色块，图片显示缩略图，其他显示类型图标
fn kind_badge(clip: &Clip, p: &Palette, size: Pixels) -> Div {
    let base = div()
        .size(size)
        .flex_none()
        .rounded(design::RADIUS_MD)
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center();
    if clip.kind == Kind::Color
        && let Some(color) = parse_hex_color(&clip.content)
    {
        return base.bg(color).border_1().border_color(p.border);
    }
    if let Some(img_info) = &clip.image {
        return base.border_1().border_color(p.border).child(
            img(img_info.path.clone())
                .size_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    base.bg(p.kind_soft(clip.kind)).child(
        Icon::new(kind_icon(clip.kind))
            .small()
            .text_color(p.kind_color(clip.kind)),
    )
}

/// 键帽样式的快捷键提示
pub(crate) fn kbd(label: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .flex_none()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(5.))
        .rounded(px(4.))
        .bg(p.kbd_bg)
        .border_1()
        .border_color(p.kbd_border)
        .text_size(px(11.))
        .text_color(p.text_muted)
        .flex()
        .items_center()
        .justify_center()
        .child(label.into())
}

fn hint(keys: &[&'static str], label: &'static str, p: &Palette) -> Div {
    h_flex()
        .gap_1()
        .children(keys.iter().map(|k| kbd(*k, p)))
        .child(div().ml_0p5().child(label))
}

/// 查找所有匹配位置（ASCII 不区分大小写，CJK 原样匹配，字节偏移保持不变）
fn match_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let hay = text.to_ascii_lowercase();
    let mut ranges = Vec::new();
    for term in query.split_whitespace() {
        let term = term.to_ascii_lowercase();
        let mut start = 0;
        while let Some(pos) = hay[start..].find(&term) {
            let s = start + pos;
            let e = s + term.len();
            if text.is_char_boundary(s) && text.is_char_boundary(e) {
                ranges.push(s..e);
            }
            start = e;
        }
    }
    ranges.sort_by_key(|r| r.start);
    // 合并重叠区间，StyledText 要求高亮区间有序且不重叠
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

fn highlighted(text: String, query: &str, p: &Palette) -> StyledText {
    let style = HighlightStyle {
        color: Some(p.accent_text),
        background_color: Some(p.highlight_bg),
        ..Default::default()
    };
    let ranges = match_ranges(&text, query);
    StyledText::new(text).with_highlights(ranges.into_iter().map(move |r| (r, style)))
}

fn human_bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.0} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

fn preview_text(content: &str) -> String {
    if content.chars().count() > PREVIEW_CHARS {
        let head: String = content.chars().take(PREVIEW_CHARS).collect();
        format!("{head}\n\n…（内容较长，预览只显示前 {PREVIEW_CHARS} 个字符）")
    } else {
        content.to_string()
    }
}

impl Popup {
    fn render_header(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let paused = self.paused.load(Ordering::Relaxed);
        let p = *p;
        h_flex()
            .id("header")
            .h(px(64.))
            .px_4()
            .gap_3()
            // 顶部空白处可以拖动窗口；搜索框和徽标用 occlude() 挡住，点击它们不会触发拖动
            .window_control_area(WindowControlArea::Drag)
            .child(img(self.logo.clone()).size(px(26.)).flex_none())
            // 搜索框：白底圆角 + 品牌色描边和柔和光晕，表示"正在等你输入"
            .child(
                h_flex()
                    .id("search-field")
                    .occlude()
                    .flex_1()
                    .h(px(40.))
                    .pl_3()
                    .pr_2()
                    .gap_2()
                    .rounded(design::RADIUS_MD)
                    .bg(p.search_bg)
                    .border_1()
                    .border_color(p.search_border)
                    .shadow(vec![BoxShadow {
                        color: p.search_glow,
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(3.),
                        inset: false,
                    }])
                    .child(Icon::new(IconName::Search).small().text_color(p.accent))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.search).appearance(false)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(p.text_faint)
                            .child(format!("{} / {}", self.items.len(), self.counts.total)),
                    ),
            )
            // 记录状态：点击切换 暂停 / 恢复
            .child(
                h_flex()
                    .id("live-badge")
                    .occlude()
                    .flex_none()
                    .h(px(26.))
                    .px_2p5()
                    .gap_1p5()
                    .rounded_full()
                    .cursor_pointer()
                    .text_xs()
                    .map(|d| {
                        if paused {
                            d.bg(p.hover)
                                .text_color(p.text_muted)
                                .child(div().size(px(6.)).rounded_full().bg(p.text_faint))
                                .child("已暂停")
                        } else {
                            d.bg(p.live_bg)
                                .text_color(p.live_text)
                                .child(div().size(px(6.)).rounded_full().bg(p.live_dot))
                                .child("记录中")
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.toggle_pause(cx)),
                    ),
            )
            .child(
                div()
                    .id("open-settings")
                    .occlude()
                    .flex_none()
                    .size(px(30.))
                    .rounded(design::RADIUS_MD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(p.text_muted)
                    .hover(move |s| s.bg(p.hover).text_color(p.text))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.open_settings(cx)),
                    )
                    .child(Icon::new(IconName::Settings).small()),
            )
    }

    fn render_filters(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        h_flex()
            .id("filters")
            .px_3()
            .pb_2()
            .gap_1()
            .window_control_area(WindowControlArea::Drag)
            .children(Filter::ALL.iter().enumerate().map(|(i, filter)| {
                let active = i == self.filter_ix;
                let count = self.counts.get(*filter);
                h_flex()
                    .id(("filter", i))
                    .occlude()
                    .gap_1()
                    .px_2p5()
                    .py_1()
                    .rounded(design::RADIUS_SM)
                    .text_xs()
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(p.accent_soft)
                            .text_color(p.accent_text)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!active, |d| {
                        d.text_color(p.text_muted).hover(move |s| s.bg(p.hover))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.select_filter(i, cx)),
                    )
                    .child(filter.label())
                    .when(i != 0, |d| {
                        d.child(
                            div()
                                .text_color(if active { p.accent_text } else { p.text_faint })
                                .child(count.to_string()),
                        )
                    })
            }))
    }

    fn render_row(&self, pos: usize, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        let clip = &self.items[pos];
        let is_selected = pos == self.selected;
        let queue_pos = self.queue.iter().position(|u| *u == clip.uuid);
        let meta = format!(
            "{} · {}",
            clip.source.as_deref().unwrap_or("未知来源"),
            relative_time(clip.last_used_at, self.now)
        );

        h_flex()
            .id(("row", clip.seq as usize))
            .relative()
            .h(design::ROW_HEIGHT)
            // 虚拟列表不会把列表宽度传给行，这里显式给出（左右各留 8px 外边距）
            .w(design::LIST_WIDTH - px(16.))
            .flex_none()
            .mx_2()
            .px_2()
            .gap_3()
            .rounded(design::RADIUS_MD)
            .cursor_pointer()
            .when(is_selected, |d| {
                d.bg(p.selected).child(
                    div()
                        .absolute()
                        .left(px(-2.))
                        .top(px(16.))
                        .bottom(px(16.))
                        .w(px(3.))
                        .rounded_full()
                        .bg(p.accent),
                )
            })
            .when(!is_selected, |d| d.hover(move |s| s.bg(p.hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if this.editing.is_some() {
                        this.status = Some("正在编辑：Ctrl+S 保存，Esc 放弃修改".into());
                        cx.notify();
                    } else if event.click_count >= 2 {
                        this.paste(pos, false, cx);
                    } else {
                        this.selected = pos;
                        cx.notify();
                    }
                }),
            )
            .child(kind_badge(clip, &p, px(32.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.text)
                            .truncate()
                            .child(highlighted(clip.title(), &self.highlight, &p)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.text_faint)
                            .truncate()
                            .child(meta),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1p5()
                    .when_some(queue_pos, |d, n| {
                        d.child(
                            h_flex()
                                .flex_none()
                                .h(px(18.))
                                .px_1()
                                .gap_0p5()
                                .rounded(px(4.))
                                .bg(p.accent_soft)
                                .text_color(p.accent_text)
                                .text_size(px(11.))
                                .child(Icon::new(Lucide::ListOrdered).xsmall())
                                .child(format!("{}", n + 1)),
                        )
                    })
                    .when(clip.pinned, |d| {
                        d.child(
                            Icon::new(IconName::StarFill)
                                .xsmall()
                                .text_color(rgb(0xF5A524)),
                        )
                    })
                    .when(pos < 9, |d| d.child(kbd(format!("{}", pos + 1), &p))),
            )
            .into_any_element()
    }

    fn render_empty(&self, p: &Palette) -> impl IntoElement {
        let (title, subtitle) = if self.counts.total == 0 {
            ("还没有任何记录", "复制任意内容，它就会出现在这里")
        } else {
            (
                "没有找到匹配的内容",
                "换个关键词、按 Tab 切换分类，或用 app:  type:  is:  -词 过滤",
            )
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(p.text_faint)
            .child(Icon::new(IconName::Inbox).large())
            .child(div().text_sm().child(title))
            .child(div().text_xs().child(subtitle))
    }

    fn render_preview(&self, clip: &Clip, p: &Palette) -> impl IntoElement {
        let mono = "Cascadia Mono";
        let body: AnyElement = match clip.kind {
            Kind::Text => div()
                .text_base()
                .line_height(relative(1.65))
                .text_color(p.text)
                .child(highlighted(preview_text(&clip.content), &self.highlight, p))
                .into_any_element(),
            Kind::Code => div()
                .p_3()
                .rounded(design::RADIUS_MD)
                .bg(p.code_bg)
                .border_1()
                .border_color(p.divider)
                .font_family(mono)
                .text_size(px(12.5))
                .line_height(relative(1.55))
                .text_color(p.text)
                .child(preview_text(&clip.content))
                .into_any_element(),
            Kind::Link => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text)
                        .child(clip.domain().to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(p.accent_text)
                        .child(clip.content.trim().to_string()),
                )
                .child(
                    h_flex()
                        .mt_2()
                        .gap_1p5()
                        .text_xs()
                        .text_color(p.text_muted)
                        .child(Icon::new(IconName::ExternalLink).xsmall())
                        .child("Ctrl+O 在浏览器中打开"),
                )
                .into_any_element(),
            Kind::Color => {
                let color = parse_hex_color(&clip.content).unwrap_or(p.accent);
                let rgb = gpui_kit::Rgba::from(color);
                let (r, g, b) = (
                    (rgb.r * 255.0).round() as u8,
                    (rgb.g * 255.0).round() as u8,
                    (rgb.b * 255.0).round() as u8,
                );
                let row = |label: &'static str, value: String| {
                    h_flex()
                        .justify_between()
                        .py_1p5()
                        .border_b_1()
                        .border_color(p.divider)
                        .child(div().text_color(p.text_muted).child(label))
                        .child(div().font_family(mono).text_color(p.text).child(value))
                };
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .h(px(120.))
                            .rounded(design::RADIUS_LG)
                            .bg(color)
                            .border_1()
                            .border_color(p.border),
                    )
                    .child(
                        v_flex()
                            .text_sm()
                            .child(row("原值", clip.content.trim().to_string()))
                            .child(row("HEX", format!("#{r:02X}{g:02X}{b:02X}")))
                            .child(row("RGB", format!("{r}, {g}, {b}")))
                            .child(row(
                                "HSL",
                                format!(
                                    "{:.0}°, {:.0}%, {:.0}%",
                                    color.h * 360.0,
                                    color.s * 100.0,
                                    color.l * 100.0
                                ),
                            )),
                    )
                    .into_any_element()
            }
            Kind::Image => match &clip.image {
                Some(info) => v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h(px(260.))
                            .rounded(design::RADIUS_LG)
                            .border_1()
                            .border_color(p.border)
                            .bg(p.code_bg)
                            .overflow_hidden()
                            .p_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                // GPUI 的 img 会把图片自身的宽高比设为布局的 aspect_ratio，
                                // 同时给宽和高时高度会被宽度推算覆盖，方图就被裁掉了。
                                // 所以只限制最大宽高，让图片按比例缩进框里。
                                img(info.path.clone())
                                    .max_w_full()
                                    .max_h(px(244.))
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.text_muted)
                            .child("Ctrl+O 用系统查看器打开"),
                    )
                    .into_any_element(),
                None => div().child("图片文件丢失").into_any_element(),
            },
            Kind::File => v_flex()
                .gap_1()
                .children(clip.files().map(|path| {
                    let (dir, name) = path.rsplit_once(['\\', '/']).unwrap_or(("", path));
                    let exists = std::path::Path::new(path).exists();
                    h_flex()
                        .gap_2p5()
                        .py_1p5()
                        .child(
                            Icon::new(IconName::File)
                                .small()
                                .text_color(p.kind_color(Kind::File)),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(if exists { p.text } else { p.text_faint })
                                        .child(if exists {
                                            name.to_string()
                                        } else {
                                            format!("{name}（已不存在）")
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(p.text_faint)
                                        .truncate()
                                        .child(dir.to_string()),
                                ),
                        )
                }))
                .into_any_element(),
        };

        let stats = match clip.kind {
            Kind::Image => clip
                .image
                .as_ref()
                .map(|i| {
                    format!(
                        "{} × {} · PNG · {}",
                        i.width,
                        i.height,
                        human_bytes(i.bytes)
                    )
                })
                .unwrap_or_default(),
            Kind::File => format!("{} 个文件", clip.files().count()),
            Kind::Code => format!(
                "{} · {} 行 · {} 字符",
                clip.lang.as_deref().unwrap_or("代码"),
                clip.line_count(),
                clip.char_count()
            ),
            _ => format!("{} 行 · {} 字符", clip.line_count(), clip.char_count()),
        };
        let stats = if clip.use_count > 1 {
            format!("{stats} · 复制过 {} 次", clip.use_count)
        } else {
            stats
        };
        let stats = if clip.has_rich {
            format!("{stats} · Shift+↵ 粘贴为纯文本")
        } else {
            stats
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .p_4()
            .gap_3()
            .bg(p.surface)
            .child(
                h_flex()
                    .gap_2()
                    .child(kind_badge(clip, p, px(22.)))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child(clip.kind.label()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(p.text_faint)
                            .child(format!(
                                "{} · {}",
                                clip.source.as_deref().unwrap_or("未知来源"),
                                relative_time(clip.last_used_at, self.now)
                            )),
                    )
                    .child(div().flex_1())
                    .when(clip.has_rich, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .py_0p5()
                                .rounded(design::RADIUS_SM)
                                .bg(p.accent_soft)
                                .text_xs()
                                .text_color(p.accent_text)
                                .child("含格式"),
                        )
                    })
                    .when(clip.pinned, |d| {
                        d.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .text_xs()
                                .text_color(rgb(0xD97706))
                                .child(Icon::new(IconName::StarFill).xsmall())
                                .child("已收藏"),
                        )
                    }),
            )
            .child(
                div()
                    .id("preview-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(div().text_xs().text_color(p.text_faint).child(stats))
    }

    fn render_editor(&self, p: &Palette) -> impl IntoElement {
        let target = self
            .preview
            .as_ref()
            .filter(|c| Some(&c.uuid) == self.editing.as_ref());
        let rich = target.is_some_and(|c| c.has_rich);
        v_flex()
            .flex_1()
            .min_w_0()
            .p_4()
            .gap_3()
            .bg(p.surface)
            .key_context(EDITOR_CONTEXT)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .size(px(22.))
                            .flex_none()
                            .rounded(design::RADIUS_MD)
                            .bg(p.accent_soft)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(Lucide::SquarePen).xsmall().text_color(p.accent)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child("编辑"),
                    )
                    .when_some(target, |d, clip| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(p.text_faint)
                                .child(format!(
                                    "{} · {}",
                                    clip.kind.label(),
                                    clip.source.as_deref().unwrap_or("未知来源")
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .rounded(design::RADIUS_MD)
                    .bg(p.search_bg)
                    .border_1()
                    .border_color(p.search_border)
                    .child(
                        Textarea::new(&self.editor)
                            .appearance(false)
                            .h(relative(1.)),
                    ),
            )
            .child(div().text_xs().text_color(p.text_faint).child(if rich {
                "保存后替换这条记录。原来带的格式（字体、颜色等）会去掉，只保留文字。"
            } else {
                "保存后替换这条记录，位置和收藏状态不变。"
            }))
    }

    fn render_footer(&self, p: &Palette) -> impl IntoElement {
        h_flex()
            .h(px(38.))
            .flex_none()
            .px_4()
            .gap_4()
            .border_t_1()
            .border_color(p.divider)
            .text_xs()
            .text_color(p.text_muted)
            .map(|d| {
                if self.editing.is_some() {
                    d.child(hint(&["Ctrl", "S"], "保存", p))
                        .child(hint(&["Ctrl", "↵"], "保存并粘贴", p))
                        .child(hint(&["Esc"], "放弃修改", p))
                } else {
                    d.child(hint(&["↵"], "粘贴", p))
                        .child(hint(&["Ctrl", "K"], "更多动作", p))
                        .child(hint(&["Ctrl", "E"], "编辑", p))
                        .child(hint(&["Ctrl", "Q"], "加入队列", p))
                        .child(hint(&["Tab"], "分类", p))
                }
            })
            .child(div().flex_1())
            .when(!self.queue.is_empty(), |d| {
                d.child(
                    h_flex()
                        .flex_none()
                        .h(px(20.))
                        .px_1p5()
                        .gap_1()
                        .rounded(px(4.))
                        .bg(p.accent_soft)
                        .text_color(p.accent_text)
                        .child(Icon::new(Lucide::ListOrdered).xsmall())
                        .child(format!("队列 {}", self.queue.len())),
                )
            })
            .map(|d| match (&self.config_error, &self.status) {
                (Some(err), _) => d.child(
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .text_color(rgb(0xDC2626))
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(div().truncate().child(err.clone())),
                ),
                (None, Some(status)) => d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(p.accent_text)
                        .child(status.clone()),
                ),
                (None, None) => d.child(hint(&["Esc"], "关闭", p)),
            })
    }
}

impl Render for Popup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(cx.theme().is_dark());

        // 选中项变化时读取它的完整内容（列表里的内容是截断过的）
        let selected_uuid = self.items.get(self.selected).map(|c| c.uuid.clone());
        if self.preview.as_ref().map(|c| &c.uuid) != selected_uuid.as_ref() {
            self.preview = self.full_clip(self.selected);
        }
        let preview = if self.editing.is_some() {
            Some(self.render_editor(&p).into_any_element())
        } else {
            self.preview
                .as_ref()
                .map(|clip| self.render_preview(clip, &p).into_any_element())
        };
        let empty = self.items.is_empty();
        let menu = self.menu.as_ref().map(|menu| {
            let view = cx.entity().downgrade();
            menu.render(&self.menu_search, &p, move |pos, window, cx| {
                let _ = view.update(cx, |this, cx| this.pick_menu_entry(pos, window, cx));
            })
        });

        // 虚拟列表：只渲染可见的那几行，历史再多滚动也流畅
        let list = uniform_list(
            "list",
            self.items.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|pos| this.render_row(pos, &p, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .w(design::LIST_WIDTH)
        .h_full()
        .flex_none()
        .py_2()
        .track_scroll(&self.scroll);

        let content = h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .when(empty, |d| d.child(self.render_empty(&p)))
            .when(!empty, |d| {
                d.child(list)
                    .child(div().w(px(1.)).bg(p.divider))
                    .children(preview)
            });

        v_flex()
            .size_full()
            .relative()
            .bg(p.panel)
            .text_color(p.text)
            .text_sm()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_next_filter))
            .on_action(cx.listener(Self::on_prev_filter))
            .on_action(cx.listener(Self::on_dismiss))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_paste_plain))
            .on_action(cx.listener(Self::on_toggle_pin))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_open_menu))
            .on_action(cx.listener(Self::on_menu_prev))
            .on_action(cx.listener(Self::on_menu_next))
            .on_action(cx.listener(Self::on_menu_close))
            .on_action(cx.listener(Self::on_menu_confirm))
            .on_action(cx.listener(Self::on_toggle_queue))
            .on_action(cx.listener(Self::on_start_edit))
            .on_action(cx.listener(Self::on_save_edit))
            .on_action(cx.listener(Self::on_save_edit_and_paste))
            .on_action(cx.listener(Self::on_cancel_edit))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.on_key_down(event, cx);
            }))
            .child(
                v_flex()
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(p.header_wash, 0.),
                        linear_color_stop(transparent_black(), 1.),
                    ))
                    .child(self.render_header(&p, cx))
                    .child(self.render_filters(&p, cx)),
            )
            .child(div().h(px(1.)).bg(p.divider))
            .child(content)
            .child(self.render_footer(&p))
            .when_some(menu, |d, menu| {
                d.child(
                    // 遮罩：挡住下面的列表和拖动区域，点一下关闭菜单
                    div()
                        .id("menu-scrim")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(p.scrim)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_menu(window, cx)),
                        ),
                )
                .child(menu)
            })
            .with_animation(
                ("panel-show", self.show_count),
                Animation::new(design::SHOW_ANIMATION).with_easing(ease_out_quint()),
                |el, delta| {
                    el.opacity(delta)
                        .top(px((1.0 - delta) * design::SHOW_OFFSET))
                },
            )
    }
}
