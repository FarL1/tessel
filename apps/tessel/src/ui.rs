//! 弹出面板的界面：搜索栏、分类标签、列表、预览、快捷键提示。

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_clipboard::WriteContent;
use tessel_core::time::{now_ms, relative as relative_time};
use tessel_core::{Clip, Counts, Filter, Kind, Store};

use crate::design::{self, Palette, parse_hex_color};
use crate::{debug_log, log_line, win};

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
        OpenClip
    ]
);

const CONTEXT: &str = "ClipPopup";
/// 一次最多加载的条数（列表是普通滚动列表，M2 换成虚拟列表后再放开）
const RESULT_LIMIT: usize = 300;
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
        ]);
    }
    cx.bind_keys(bindings);
}

pub struct Popup {
    hwnd: isize,
    visible: bool,
    /// 弹出前的前台窗口，粘贴时切回它
    prev_foreground: Option<isize>,
    store: Store,
    paused: Arc<AtomicBool>,
    search: Entity<InputState>,
    query: String,
    filter_ix: usize,
    items: Vec<Clip>,
    counts: Counts,
    /// 在当前结果中的位置
    selected: usize,
    /// 渲染时用的"现在"，用来算"5 分钟前"
    now: i64,
    show_count: usize,
    scroll: ScrollHandle,
    status: Option<SharedString>,
    logo: Arc<Image>,
    /// 上次自动摆放的位置；隐藏时如果窗口不在这里，说明用户拖动过
    placed_at: Option<(i32, i32)>,
    /// 用户拖动后的位置，下次唤起时沿用
    user_pos: Option<(i32, i32)>,
    _subscriptions: Vec<Subscription>,
}

impl Popup {
    pub fn new(
        hwnd: isize,
        store: Store,
        paused: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索剪贴板历史…"));

        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, state, event, _window, cx| {
                if let InputEvent::Change = event {
                    this.query = state.read(cx).value().to_string();
                    this.selected = 0;
                    this.scroll.scroll_to_item(0);
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
                    this.search.update(cx, |state, cx| state.focus(window, cx));
                }
            }),
            // 跟随系统深浅色
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
        ];

        let mut this = Self {
            hwnd,
            visible: false,
            prev_foreground: None,
            store,
            paused,
            search,
            query: String::new(),
            filter_ix: 0,
            items: Vec::new(),
            counts: Counts::default(),
            selected: 0,
            now: now_ms(),
            show_count: 0,
            scroll: ScrollHandle::new(),
            status: None,
            logo: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../../../assets/brand/png/tessel-64.png").to_vec(),
            )),
            placed_at: None,
            user_pos: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn filter(&self) -> Filter {
        Filter::ALL[self.filter_ix]
    }

    /// 按当前搜索词和分类重新查询
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        match self.store.search(&self.query, self.filter(), RESULT_LIMIT) {
            Ok(items) => self.items = items,
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

    /// 采集线程写入了新内容
    pub fn on_clipboard_changed(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            self.refresh(cx);
        }
    }

    pub fn reset_position(&mut self) {
        self.user_pos = None;
    }

    /// 托盘点击、再次启动：只负责显示，已经显示时保持不动
    pub fn show_if_hidden(&mut self, t0: Instant, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
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
        self.selected = 0;
        self.filter_ix = 0;
        self.status = None;
        self.show_count += 1;
        self.query.clear();
        self.scroll.scroll_to_item(0);
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
        win::hide(self.hwnd);
        if restore_focus && let Some(prev) = self.prev_foreground {
            win::focus(prev);
        }
        let (ws, private) = win::memory_mb();
        debug_log(format!("[hide] 工作集 {ws:.1}MB · 私有 {private:.1}MB"));
        cx.notify();
    }

    fn write_to_clipboard(&self, clip: &Clip) -> Result<(), String> {
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
            _ => tessel_clipboard::write(owner, WriteContent::Text(&clip.content)),
        }
    }

    fn paste(&mut self, pos: usize, plain: bool, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(pos).cloned() else {
            return;
        };
        let t0 = Instant::now();
        if let Err(err) = self.write_to_clipboard(&clip) {
            self.status = Some(format!("写入剪贴板失败：{err}").into());
            cx.notify();
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
            "[paste] {}{} {} → 「{}」· 切换焦点 {:.1}ms · 总耗时 {:.1}ms",
            if report.focused {
                "✅"
            } else {
                "❌ 无法切回目标窗口"
            },
            if plain { "（纯文本）" } else { "" },
            clip.kind.label(),
            report.target_title,
            report.focus_ms,
            t0.elapsed().as_secs_f64() * 1000.0,
        ));
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn select_filter(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.filter_ix = ix % Filter::ALL.len();
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        self.refresh(cx);
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let paused = !self.paused.load(Ordering::Relaxed);
        self.paused.store(paused, Ordering::Relaxed);
        self.status = Some(
            if paused {
                "已暂停记录"
            } else {
                "已恢复记录"
            }
            .into(),
        );
        cx.notify();
    }

    /// Ctrl+1–9 快速粘贴。数字键没有被其他组件绑定，用捕获阶段的按键监听即可。
    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        debug_log(format!("[event] key_down {key:?} {mods:?}"));
        if mods.control && key.len() == 1 && (b'1'..=b'9').contains(&key.as_bytes()[0]) {
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
        self.select_filter(self.filter_ix + 1, cx);
    }

    fn on_prev_filter(&mut self, _: &PrevFilter, _: &mut Window, cx: &mut Context<Self>) {
        self.select_filter(self.filter_ix + Filter::ALL.len() - 1, cx);
    }

    fn on_dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.hide(true, cx);
    }

    fn on_paste(&mut self, _: &PasteSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.paste(self.selected, false, cx);
    }

    fn on_paste_plain(&mut self, _: &PastePlainText, _: &mut Window, cx: &mut Context<Self>) {
        self.paste(self.selected, true, cx);
    }

    fn on_toggle_pin(&mut self, _: &TogglePin, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(self.selected) else {
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
        let Some(clip) = self.items.get(self.selected) else {
            return;
        };
        if let Err(err) = self.store.delete(&clip.uuid) {
            self.status = Some(format!("删除失败：{err}").into());
        } else {
            self.status = Some("已删除".into());
        }
        self.refresh(cx);
    }

    fn on_open(&mut self, _: &OpenClip, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(self.selected) else {
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
fn kbd(label: impl Into<SharedString>, p: &Palette) -> Div {
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
        let meta = format!(
            "{} · {}",
            clip.source.as_deref().unwrap_or("未知来源"),
            relative_time(clip.last_used_at, self.now)
        );

        h_flex()
            .id(("row", clip.seq as usize))
            .relative()
            .h(design::ROW_HEIGHT)
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
                    if event.click_count >= 2 {
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
                            .child(highlighted(clip.title(), &self.query, &p)),
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
            ("没有找到匹配的内容", "换个关键词，或按 Tab 切换分类")
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
                .child(highlighted(preview_text(&clip.content), &self.query, p))
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
                            .child(
                                img(info.path.clone())
                                    .size_full()
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
            .child(hint(&["↵"], "粘贴", p))
            .child(hint(&["Ctrl", "1–9"], "快速粘贴", p))
            .child(hint(&["Ctrl", "P"], "收藏", p))
            .child(hint(&["Shift", "Del"], "删除", p))
            .child(hint(&["Tab"], "分类", p))
            .child(div().flex_1())
            .map(|d| match &self.status {
                Some(status) => d.child(div().text_color(p.accent_text).child(status.clone())),
                None => d.child(hint(&["Esc"], "关闭", p)),
            })
    }
}

impl Render for Popup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(cx.theme().is_dark());
        let rows: Vec<AnyElement> = (0..self.items.len())
            .map(|pos| self.render_row(pos, &p, cx))
            .collect();
        let preview = self
            .items
            .get(self.selected)
            .map(|clip| self.render_preview(clip, &p).into_any_element());
        let empty = self.items.is_empty();

        let content = h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .when(empty, |d| d.child(self.render_empty(&p)))
            .when(!empty, |d| {
                d.child(
                    div()
                        .id("list")
                        .w(design::LIST_WIDTH)
                        .flex_none()
                        .py_2()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .children(rows),
                )
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
