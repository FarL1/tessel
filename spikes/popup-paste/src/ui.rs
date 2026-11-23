//! 弹出面板的界面：搜索栏、分类标签、列表、预览、快捷键提示。

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::data::{Clip, FILTERS, Kind, sample_clips};
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
        PastePlainText
    ]
);

const CONTEXT: &str = "ClipPopup";

/// 弹窗的键位表。正式版本里会从用户配置文件加载，允许自定义。
///
/// 谓词写成 `ClipPopup > Input`：焦点在搜索框里时，搜索框自身也绑定了 up/down/enter/tab，
/// 同一深度的绑定以后注册的为准，所以这些绑定必须在 `gpui_kit::init` 之后注册。
pub fn bind_keys(cx: &mut App) {
    let in_popup = Some("ClipPopup > Input");
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, in_popup),
        KeyBinding::new("down", SelectNext, in_popup),
        KeyBinding::new("tab", NextFilter, in_popup),
        KeyBinding::new("shift-tab", PrevFilter, in_popup),
        KeyBinding::new("escape", Dismiss, in_popup),
        KeyBinding::new("enter", PasteSelected, in_popup),
        KeyBinding::new("shift-enter", PastePlainText, in_popup),
        KeyBinding::new("up", SelectPrev, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("tab", NextFilter, Some(CONTEXT)),
        KeyBinding::new("shift-tab", PrevFilter, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
        KeyBinding::new("enter", PasteSelected, Some(CONTEXT)),
        KeyBinding::new("shift-enter", PastePlainText, Some(CONTEXT)),
    ]);
}

pub struct Popup {
    hwnd: isize,
    visible: bool,
    /// 弹出前的前台窗口，粘贴时切回它
    prev_foreground: Option<isize>,
    search: Entity<InputState>,
    query: String,
    clips: Vec<Clip>,
    filter_ix: usize,
    /// 在当前可见结果中的位置
    selected: usize,
    show_count: usize,
    scroll: ScrollHandle,
    status: Option<SharedString>,
    logo: Arc<Image>,
    _subscriptions: Vec<Subscription>,
}

impl Popup {
    pub fn new(hwnd: isize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索剪贴板历史…"));

        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, state, event, _window, cx| {
                if let InputEvent::Change = event {
                    this.query = state.read(cx).value().to_string();
                    this.selected = 0;
                    this.scroll.scroll_to_item(0);
                    cx.notify();
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

        Self {
            hwnd,
            visible: false,
            prev_foreground: None,
            search,
            query: String::new(),
            clips: sample_clips(),
            filter_ix: 0,
            selected: 0,
            show_count: 0,
            scroll: ScrollHandle::new(),
            status: None,
            logo: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../../../assets/brand/png/tessel-64.png").to_vec(),
            )),
            _subscriptions: subscriptions,
        }
    }

    /// 当前筛选和搜索条件下可见的条目（clips 的下标）
    fn visible_items(&self) -> Vec<usize> {
        let filter = FILTERS[self.filter_ix];
        let terms: Vec<String> = self
            .query
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        self.clips
            .iter()
            .enumerate()
            .filter(|(_, clip)| filter.matches(clip))
            .filter(|(_, clip)| {
                let hay = format!("{} {}", clip.content, clip.source).to_ascii_lowercase();
                terms.iter().all(|t| hay.contains(t.as_str()))
            })
            .map(|(i, _)| i)
            .collect()
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
        self.scroll.scroll_to_item(0);

        // 出现在鼠标所在显示器的偏上位置（类似 Spotlight）
        win::center_on_cursor_monitor(self.hwnd);
        // 注意：不要调用 window.activate_window()。GPUI 在 Windows 上的实现是异步的，
        // 会重新应用创建时的窗口位置状态，还会注入一次 Alt 键。收到 WM_HOTKEY 时我们本身就有前台权限。
        if !win::show_and_focus(self.hwnd) {
            log_line("[show] ⚠ SetFocus 之后键盘焦点仍不在弹窗上".into());
        }
        self.search.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        self.query.clear();

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

    /// `restore_focus`：Esc / 热键关闭时把焦点还给之前的窗口；点击外部时不需要
    fn hide(&mut self, restore_focus: bool, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        win::hide(self.hwnd);
        if restore_focus && let Some(prev) = self.prev_foreground {
            win::focus(prev);
        }
        let (ws, private) = win::memory_mb();
        log_line(format!("[hide] 工作集 {ws:.1}MB · 私有 {private:.1}MB"));
        cx.notify();
    }

    fn paste(&mut self, clip_ix: usize, plain: bool, cx: &mut Context<Self>) {
        let text = self.clips[clip_ix].content;
        let Some(target) = self.prev_foreground.take() else {
            let _ = win::set_clipboard_text(self.hwnd, text);
            self.status = Some("已复制到剪贴板（没有记录到来源窗口）".into());
            cx.notify();
            return;
        };

        let t0 = Instant::now();
        if let Err(err) = win::set_clipboard_text(self.hwnd, text) {
            self.status = Some(format!("写入剪贴板失败：{err}").into());
            cx.notify();
            return;
        }
        self.visible = false;
        win::hide(self.hwnd);
        let report = win::restore_and_paste(target);
        log_line(format!(
            "[paste] {}{} 目标「{}」· 切换焦点 {:.1}ms · 总耗时 {:.1}ms",
            if report.focused {
                "✅"
            } else {
                "❌ 无法切回目标窗口"
            },
            if plain { "（纯文本）" } else { "" },
            report.target_title,
            report.focus_ms,
            t0.elapsed().as_secs_f64() * 1000.0,
        ));
        cx.notify();
    }

    fn paste_nth(&mut self, nth: usize, plain: bool, cx: &mut Context<Self>) {
        if let Some(&clip_ix) = self.visible_items().get(nth) {
            self.paste(clip_ix, plain, cx);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.visible_items().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn select_filter(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.filter_ix = ix % FILTERS.len();
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    /// Ctrl+1–9 快速粘贴。数字键没有被其他组件绑定，用捕获阶段的按键监听即可。
    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        debug_log(format!("[event] key_down {key:?} {mods:?}"));
        if mods.control && key.len() == 1 && (b'1'..=b'9').contains(&key.as_bytes()[0]) {
            self.paste_nth((key.as_bytes()[0] - b'1') as usize, mods.shift, cx);
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
        self.select_filter(self.filter_ix + FILTERS.len() - 1, cx);
    }

    fn on_dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.hide(true, cx);
    }

    fn on_paste(&mut self, _: &PasteSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_nth(self.selected, false, cx);
    }

    fn on_paste_plain(&mut self, _: &PastePlainText, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_nth(self.selected, true, cx);
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

/// 列表左侧的类型徽标；颜色类型直接显示色块
fn kind_badge(clip: &Clip, p: &Palette, size: Pixels) -> Div {
    let base = div()
        .size(size)
        .flex_none()
        .rounded(design::RADIUS_MD)
        .flex()
        .items_center()
        .justify_center();
    if clip.kind == Kind::Color
        && let Some(color) = parse_hex_color(clip.content)
    {
        return base.bg(color).border_1().border_color(p.border);
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

impl Popup {
    fn render_header(&self, p: &Palette, shown: usize) -> impl IntoElement {
        h_flex()
            .h(px(64.))
            .px_4()
            .gap_3()
            // 品牌 Logo
            .child(img(self.logo.clone()).size(px(26.)).flex_none())
            // 搜索框：白底圆角 + 品牌色描边和柔和光晕，表示"正在等你输入"
            .child(
                h_flex()
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
                            .child(format!("{shown} / {}", self.clips.len())),
                    ),
            )
            // 状态：正在记录剪贴板
            .child(
                h_flex()
                    .flex_none()
                    .h(px(26.))
                    .px_2p5()
                    .gap_1p5()
                    .rounded_full()
                    .bg(p.live_bg)
                    .text_xs()
                    .text_color(p.live_text)
                    .child(div().size(px(6.)).rounded_full().bg(p.live_dot))
                    .child("记录中"),
            )
    }

    fn render_filters(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        h_flex()
            .px_3()
            .pb_2()
            .gap_1()
            .children(FILTERS.iter().enumerate().map(|(i, filter)| {
                let active = i == self.filter_ix;
                let count = self.clips.iter().filter(|c| filter.matches(c)).count();
                h_flex()
                    .id(("filter", i))
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

    fn render_row(
        &self,
        pos: usize,
        clip_ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = *p;
        let clip = &self.clips[clip_ix];
        let is_selected = pos == self.selected;

        h_flex()
            .id(("row", clip_ix))
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
                        this.paste(clip_ix, false, cx);
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
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(p.text_faint)
                            .child(clip.source)
                            .child("·")
                            .child(clip.ago),
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
    }

    fn render_empty(&self, p: &Palette) -> impl IntoElement {
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(p.text_faint)
            .child(Icon::new(IconName::Inbox).large())
            .child(div().text_sm().child("没有找到匹配的内容"))
            .child(div().text_xs().child("换个关键词，或按 Tab 切换分类"))
    }

    fn render_preview(&self, clip: &Clip, p: &Palette) -> impl IntoElement {
        let mono = "Cascadia Mono";
        let body: AnyElement = match clip.kind {
            Kind::Text => div()
                .text_base()
                .line_height(relative(1.65))
                .text_color(p.text)
                .child(highlighted(clip.content.to_string(), &self.query, p))
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
                .child(clip.content)
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
                        .child(clip.content),
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
                let color = parse_hex_color(clip.content).unwrap_or(p.accent);
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
                            .child(row("HEX", clip.content.to_string()))
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
            Kind::Image => v_flex()
                .gap_2()
                .child(
                    div()
                        .h(px(190.))
                        .rounded(design::RADIUS_LG)
                        .border_1()
                        .border_color(p.border)
                        .bg(linear_gradient(
                            135.,
                            linear_color_stop(rgb(0x34D399), 0.),
                            linear_color_stop(rgb(0x3B82F6), 1.),
                        ))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Frame)
                                .large()
                                .text_color(rgba(0xFFFFFFCC)),
                        ),
                )
                .child(div().text_sm().text_color(p.text).child(clip.content))
                .child(div().text_xs().text_color(p.text_muted).child(clip.meta))
                .into_any_element(),
            Kind::File => v_flex()
                .gap_1()
                .children(clip.content.lines().map(|path| {
                    let (dir, name) = path.rsplit_once(['\\', '/']).unwrap_or(("", path));
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
                                .child(div().text_sm().text_color(p.text).child(name.to_string()))
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
            Kind::Image => clip.meta.to_string(),
            Kind::File => format!("{} 个文件", clip.line_count()),
            Kind::Code => format!(
                "{} · {} 行 · {} 字符",
                clip.meta,
                clip.line_count(),
                clip.char_count()
            ),
            _ => format!("{} 行 · {} 字符", clip.line_count(), clip.char_count()),
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
                            .text_xs()
                            .text_color(p.text_faint)
                            .child(format!("{} · {}", clip.source, clip.ago)),
                    )
                    .child(div().flex_1())
                    .when(clip.pinned, |d| {
                        d.child(
                            h_flex()
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
            .child(hint(&["Shift", "↵"], "纯文本", p))
            .child(hint(&["Ctrl", "1–9"], "快速粘贴", p))
            .child(hint(&["Tab"], "切换分类", p))
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
        let items = self.visible_items();
        if !items.is_empty() && self.selected >= items.len() {
            self.selected = items.len() - 1;
        }
        let rows: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(pos, &clip_ix)| self.render_row(pos, clip_ix, &p, cx).into_any_element())
            .collect();
        let preview = items
            .get(self.selected)
            .map(|&ix| self.render_preview(&self.clips[ix], &p).into_any_element());

        let content = h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .when(items.is_empty(), |d| d.child(self.render_empty(&p)))
            .when(!items.is_empty(), |d| {
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
                    .child(self.render_header(&p, items.len()))
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
