//! 安装和卸载界面共用的部分：颜色、图标、标题栏、底栏、进度条、打开窗口。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{Icon, Sizable, Theme, TitleBar};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

// 默认图标包之外用到的 Lucide 图标
gpui_kit::assets::icon_assets!(
    SetupIcons,
    [ClipboardClock, Keyboard, ShieldCheck, X, Trash]
);

/// 组件库自带图标 + 安装程序额外用到的图标
pub struct SetupAssets;

impl AssetSource for SetupAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = SetupIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::Assets.list(path)?;
        all.extend(SetupIcons.list(path)?);
        Ok(all)
    }
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// "进行中"页面至少停留这么久，太快的话一闪而过像是出了错
pub const MIN_BUSY: Duration = Duration::from_millis(1200);

#[derive(Clone, Copy)]
pub struct Colors {
    pub panel: Hsla,
    pub surface: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_text: Hsla,
    pub hero_from: Hsla,
    pub hero_to: Hsla,
    pub kbd_bg: Hsla,
    pub track: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub close_hover: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

impl Colors {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                panel: c(0x1C1D22E6),
                surface: c(0xFFFFFF0A),
                border: c(0xFFFFFF1A),
                text: c(0xEDEDF2FF),
                muted: c(0xA3A5AFFF),
                faint: c(0x6E717BFF),
                accent: c(0x8B9BFFFF),
                accent_soft: c(0x8B9BFF2E),
                accent_text: c(0xC9D0FFFF),
                hero_from: c(0x6D5DF640),
                hero_to: c(0x6D5DF600),
                kbd_bg: c(0xFFFFFF0F),
                track: c(0xFFFFFF14),
                success: c(0x22C55EFF),
                danger: c(0xF87171FF),
                danger_soft: c(0xF871711F),
                close_hover: c(0xC42B1CFF),
            }
        } else {
            Self {
                panel: c(0xF8F8FAEB),
                surface: c(0xFFFFFFB3),
                border: c(0x00000014),
                text: c(0x1B1C20FF),
                muted: c(0x5D5F68FF),
                faint: c(0x9497A1FF),
                accent: c(0x4F6BEDFF),
                accent_soft: c(0x4F6BED1A),
                accent_text: c(0x3550D6FF),
                hero_from: c(0xC4B5FD66),
                hero_to: c(0xC4B5FD00),
                kbd_bg: c(0xFFFFFFCC),
                track: c(0x0000000F),
                success: c(0x16A34AFF),
                danger: c(0xDC2626FF),
                danger_soft: c(0xDC26261A),
                close_hover: c(0xC42B1CFF),
            }
        }
    }
}

pub fn logo() -> Arc<Image> {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../../assets/brand/png/tessel-128.png").to_vec(),
    ))
}

/// 标题栏：拖动移动窗口，右边一个关闭按钮（不能调大小，所以没有最大化）
pub fn title_bar(
    title: &'static str,
    logo: &Arc<Image>,
    close_enabled: bool,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    p: &Colors,
) -> impl IntoElement {
    let p = *p;
    h_flex()
        .h(px(36.))
        .flex_none()
        .pl_3()
        .gap_2()
        .child(
            h_flex()
                .id("drag")
                .flex_1()
                .h_full()
                .gap_2()
                .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                .child(img(logo.clone()).size(px(16.)))
                .child(div().text_xs().text_color(p.muted).child(title)),
        )
        .child(
            div()
                .id("close")
                .w(px(46.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.muted)
                .when(!close_enabled, |d| d.opacity(0.4))
                .when(close_enabled, |d| {
                    d.hover(move |s| s.bg(p.close_hover).text_color(white()))
                })
                .child(Icon::new(Lucide::X).small())
                .on_click(on_close),
        )
}

pub fn footer(left: impl IntoElement, right: impl IntoElement, p: &Colors) -> impl IntoElement {
    h_flex()
        .flex_none()
        .h(px(64.))
        .px_8()
        .border_t_1()
        .border_color(p.border)
        .justify_between()
        .child(left)
        .child(right)
}

pub fn keycap(label: String, p: &Colors) -> Div {
    div()
        .h(px(22.))
        .min_w(px(22.))
        .px_1p5()
        .rounded(px(5.))
        .bg(p.kbd_bg)
        .border_1()
        .border_color(p.border)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.text)
        .flex()
        .items_center()
        .justify_center()
        .child(label)
}

/// 进行中的页面：Logo 呼吸 + 不确定进度的进度条（Velopack 的静默模式不报告进度，通常几秒钟就完成）
pub fn busy_page(
    logo: &Arc<Image>,
    title: &'static str,
    hint: &'static str,
    p: &Colors,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_4()
        .pb_10()
        .child(
            img(logo.clone()).size(px(72.)).with_animation(
                "breathe",
                Animation::new(Duration::from_millis(1600))
                    .repeat()
                    .with_easing(bounce(ease_in_out)),
                |el, t| el.opacity(0.65 + 0.35 * t),
            ),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text)
                .child(title),
        )
        .child(
            div()
                .relative()
                .w(px(300.))
                .h(px(6.))
                .rounded_full()
                .bg(p.track)
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(px(110.))
                        .rounded_full()
                        .bg(p.accent)
                        .with_animation(
                            "progress",
                            Animation::new(Duration::from_millis(1300))
                                .repeat()
                                .with_easing(ease_in_out),
                            |el, t| el.left(px(-110. + 410. * t)),
                        ),
                ),
        )
        .child(div().text_xs().text_color(p.faint).child(hint))
}

/// 一行带图标的提示（蓝色：说明；红色：错误）
pub fn notice(text: String, error: bool, p: &Colors) -> impl IntoElement {
    let (bg, fg) = if error {
        (p.danger_soft, p.danger)
    } else {
        (p.accent_soft, p.accent)
    };
    h_flex()
        .gap_2()
        .items_start()
        .px_3()
        .py_2()
        .rounded(px(8.))
        .bg(bg)
        .text_xs()
        .text_color(p.text)
        .child(
            div().pt(px(1.)).child(
                Icon::new(if error {
                    gpui_kit::component::IconName::TriangleAlert
                } else {
                    gpui_kit::component::IconName::Info
                })
                .xsmall()
                .text_color(fg),
            ),
        )
        .child(div().flex_1().min_w_0().child(text))
}

/// 页面外框：背景、顶部晕染、标题栏、内容淡入
pub fn frame(page_key: &'static str, bar: impl IntoElement, body: AnyElement, p: &Colors) -> Div {
    v_flex()
        .size_full()
        .bg(p.panel)
        .text_color(p.text)
        // 顶部淡淡的品牌色晕染，和启动画面呼应
        .bg(linear_gradient(
            180.,
            linear_color_stop(p.hero_from, 0.),
            linear_color_stop(p.hero_to, 0.45),
        ))
        .child(bar)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .child(div().size_full().child(body).with_animation(
                    page_key,
                    Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
                    |el, t| el.opacity(t),
                )),
        )
}

/// 组件库的主色（主按钮、复选框）换成品牌色，和 Tessel 本体一致
fn apply_brand(cx: &mut App) {
    let dark = Theme::global(cx).is_dark();
    let accent = Colors::new(dark).accent;
    let shade = |delta: f32| {
        let mut c = accent;
        c.l = (c.l + delta).clamp(0.0, 1.0);
        c
    };
    let theme = Theme::global_mut(cx);
    let (primary, hover, active) = if dark {
        (shade(-0.12), shade(-0.06), shade(-0.16))
    } else {
        (accent, shade(0.05), shade(-0.06))
    };
    let fg = white();
    theme.primary = primary;
    theme.primary_hover = hover;
    theme.primary_active = active;
    theme.primary_foreground = fg;
    theme.ring = accent;
    // 组件实际读取的是由颜色推导出的 tokens，要一起改
    theme.tokens.primary = primary.into();
    theme.tokens.primary_hover = hover.into();
    theme.tokens.primary_active = active.into();
    theme.tokens.primary_foreground = fg.into();
    theme.tokens.button_primary = primary.into();
    theme.tokens.button_primary_hover = hover.into();
    theme.tokens.button_primary_active = active.into();
    theme.tokens.button_primary_foreground = fg.into();
    theme.tokens.ring = accent.into();
}

/// 有"关闭"行为的界面：窗口的关闭（Alt+F4、任务栏）也交给它处理
pub trait Closable: Render {
    fn focus_handle(&self) -> FocusHandle;
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>);
}

/// 在屏幕中间打开一个不能调大小的窗口，并把键盘焦点放进去
pub fn open<V: Closable + 'static>(
    title: &'static str,
    size_px: (f32, f32),
    cx: &mut App,
    build: impl FnOnce(&mut Context<V>) -> V + 'static,
) {
    let bounds = Bounds::centered(None, size(px(size_px.0), px(size_px.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitleBar::title_bar_options()),
        kind: WindowKind::Normal,
        is_resizable: false,
        is_minimizable: true,
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        app_id: Some("Tessel.Setup".into()),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title(title);
        Theme::sync_system_appearance(Some(window), cx);
        apply_brand(cx);
        let view = cx.new(build);
        let focus = view.read(cx).focus_handle();
        window.focus(&focus, cx);
        let weak = view.downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(view) = weak.upgrade() {
                view.update(cx, |this, cx| this.close(window, cx));
            }
            false
        });
        view
    });
    if opened.is_err() {
        cx.quit();
    }
}
