//! 复制提示：屏幕底部中间的小胶囊，显示一下就淡出。
//!
//! 是一个独立的小窗口：不抢焦点（WS_EX_NOACTIVATE）、鼠标可以点穿、不出现在任务栏里，
//! 所以不会打断正在进行的输入。粘贴队列的进度也用它来提示。

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_core::i18n::tr;
use tessel_core::trf;
use tessel_core::{Clip, Kind};

use crate::design::{Palette, parse_hex_color};
use crate::win;

pub const WINDOW_W: f32 = 460.0;
pub const WINDOW_H: f32 = 84.0;
/// 完整显示的时长（不含淡入淡出）
const HOLD: Duration = Duration::from_millis(1500);
const ENTER: Duration = Duration::from_millis(200);
const LEAVE: Duration = Duration::from_millis(220);

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Success,
    Info,
    Warning,
}

/// 一条提示的内容
#[derive(Clone)]
pub struct Notice {
    pub tone: Tone,
    pub icon: IconName,
    pub title: SharedString,
    /// 标题旁边的小标签（类型、队列进度）
    pub tag: Option<SharedString>,
    pub detail: SharedString,
    /// 颜色记录显示一个色块
    pub swatch: Option<Hsla>,
}

impl Notice {
    pub fn new(tone: Tone, icon: IconName, title: impl Into<SharedString>) -> Self {
        Self {
            tone,
            icon,
            title: title.into(),
            tag: None,
            detail: SharedString::default(),
            swatch: None,
        }
    }

    pub fn tag(mut self, tag: impl Into<SharedString>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = detail.into();
        self
    }

    /// "已复制"：类型 + 内容摘要
    pub fn copied(clip: &Clip) -> Self {
        let mut notice = Self::new(Tone::Success, IconName::Check, tr("已复制", "Copied"))
            .tag(clip.kind.label())
            .detail(summary(clip));
        if clip.kind == Kind::Color {
            notice.swatch = parse_hex_color(&clip.content);
        }
        notice
    }
}

/// 一行内容摘要
pub fn summary(clip: &Clip) -> String {
    const MAX: usize = 36;
    match clip.kind {
        Kind::Image => clip
            .image
            .as_ref()
            .map(|i| format!("{} × {}", i.width, i.height))
            .unwrap_or_default(),
        Kind::File => {
            let files: Vec<&str> = clip.files().collect();
            let first = files
                .first()
                .map(|p| tessel_core::model::file_name(p))
                .unwrap_or_default();
            if files.len() > 1 {
                trf!("{first} 等 {} 个文件", "{first} · {} files", files.len())
            } else {
                first.to_string()
            }
        }
        _ => {
            let line = clip
                .content
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or_default();
            let mut s: String = line.chars().take(MAX).collect();
            let lines = clip.line_count();
            if line.chars().count() > MAX || lines > 1 {
                s.push('…');
            }
            if lines > 1 {
                trf!("{s}  · {lines} 行", "{s}  · {lines} lines")
            } else {
                s
            }
        }
    }
}

pub struct Toast {
    hwnd: isize,
    notice: Option<Notice>,
    /// 每显示一次加一：新提示顶掉旧提示时，旧提示的定时器失效
    generation: usize,
    leaving: bool,
}

impl Toast {
    pub fn new(hwnd: isize) -> Self {
        Self {
            hwnd,
            notice: None,
            generation: 0,
            leaving: false,
        }
    }

    pub fn show(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.generation += 1;
        self.leaving = false;
        self.notice = Some(notice);
        win::place_bottom_center(self.hwnd);
        win::show_no_activate(self.hwnd);
        cx.notify();

        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ENTER + HOLD).await;
            let still_current = this
                .update(cx, |t, cx| {
                    if t.generation == generation {
                        t.leaving = true;
                        cx.notify();
                    }
                    t.generation == generation
                })
                .unwrap_or(false);
            if !still_current {
                return;
            }
            cx.background_executor().timer(LEAVE).await;
            let _ = this.update(cx, |t, cx| {
                if t.generation == generation {
                    win::hide(t.hwnd);
                    t.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for Toast {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(cx.theme().is_dark());
        let root = div().size_full().flex().items_center().justify_center();
        let Some(notice) = &self.notice else {
            return root.into_any_element();
        };
        let (tint, tint_soft) = match notice.tone {
            Tone::Success => (rgb(0x16A34A).into(), rgba(0x16A34A26).into()),
            Tone::Info => (p.accent, p.accent_soft),
            Tone::Warning => (rgb(0xD97706).into(), rgba(0xD9770626).into()),
        };
        let badge: Hsla = tint;
        let pill = h_flex()
            .relative()
            .max_w(px(WINDOW_W - 40.))
            .h(px(48.))
            .pl(px(8.))
            .pr(px(18.))
            .gap_2p5()
            .rounded_full()
            .bg(p.popover)
            .border_1()
            .border_color(p.border)
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., if p.dark { 0.45 } else { 0.16 }),
                offset: point(px(0.), px(6.)),
                blur_radius: px(18.),
                spread_radius: px(-4.),
                inset: false,
            }])
            .child(match notice.swatch {
                Some(color) => div()
                    .size(px(32.))
                    .flex_none()
                    .rounded_full()
                    .bg(color)
                    .border_1()
                    .border_color(p.border),
                None => div()
                    .size(px(32.))
                    .flex_none()
                    .rounded_full()
                    .bg(tint_soft)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(notice.icon).small().text_color(badge)),
            })
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child(notice.title.clone()),
                            )
                            .when_some(notice.tag.clone(), |d, tag| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded(px(4.))
                                        .bg(p.hover)
                                        .text_xs()
                                        .text_color(p.text_muted)
                                        .child(tag),
                                )
                            }),
                    )
                    .when(!notice.detail.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(p.text_muted)
                                .truncate()
                                .child(notice.detail.clone()),
                        )
                    }),
            );

        let pill = if self.leaving {
            pill.with_animation(
                ("toast-leave", self.generation),
                Animation::new(LEAVE).with_easing(ease_in_out),
                |el, t| el.opacity(1.0 - t).top(px(t * 6.)),
            )
            .into_any_element()
        } else {
            pill.with_animation(
                ("toast-enter", self.generation),
                Animation::new(ENTER).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).top(px((1.0 - t) * 12.)),
            )
            .into_any_element()
        };
        root.child(pill).into_any_element()
    }
}

/// 创建提示窗口（隐藏状态）
pub fn open(cx: &mut App) -> Option<Entity<Toast>> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let bounds = Bounds::centered(None, size(px(WINDOW_W), px(WINDOW_H)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        kind: WindowKind::PopUp,
        // 和主弹窗一样：以 show: true 创建，在构建闭包里立刻隐藏（首帧之前，不会闪烁）
        show: true,
        focus: false,
        is_resizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    // 不用 gpui_kit::open_window：它外面包的 Root 会铺一层主题背景色，窗口就不透明了。
    // 提示里没有输入框、弹出层，不需要 Root。
    cx.open_window(options, |window, cx| {
        let hwnd = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            _ => panic!("expected a Win32 window handle"),
        };
        win::hide(hwnd);
        win::make_overlay(hwnd);
        win::set_title(hwnd, "Tessel 提示");
        cx.new(|_| Toast::new(hwnd))
    })
    .and_then(|handle| handle.entity(cx))
    .inspect_err(|err| crate::log_line(format!("[warn] 创建提示窗口失败：{err}")))
    .ok()
}
