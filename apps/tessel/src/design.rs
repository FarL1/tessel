//! 设计令牌：颜色、尺寸、动效时长。所有界面数值都从这里取，方便统一调整和换肤。
//!
//! 风格方向：贴近 Windows 11 Fluent（半透明材质、圆角、克制的强调色），
//! 但用自己的配色和排版，不照搬系统控件。

use std::time::Duration;

use gpui_kit::{Hsla, Pixels, px, rgba};

use tessel_core::Kind;
use tessel_core::classify::parse_hex_rgba;

/// 动效：出现时淡入 + 轻微上滑
pub const SHOW_ANIMATION: Duration = Duration::from_millis(160);
pub const SHOW_OFFSET: f32 = 10.0;

/// 窗口逻辑尺寸
pub const WINDOW_W: f32 = 780.0;
pub const WINDOW_H: f32 = 520.0;

pub const RADIUS_LG: Pixels = px(12.);
pub const RADIUS_MD: Pixels = px(8.);
pub const RADIUS_SM: Pixels = px(6.);

pub const LIST_WIDTH: Pixels = px(330.);
pub const ROW_HEIGHT: Pixels = px(54.);

fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// 整个面板的底色（半透明，让 Mica 透出来）
    pub panel: Hsla,
    /// 预览区等次级表面
    pub surface: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub border: Hsla,
    pub divider: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_text: Hsla,
    pub highlight_bg: Hsla,
    pub kbd_bg: Hsla,
    pub kbd_border: Hsla,
    pub code_bg: Hsla,
    /// 顶部区域从上往下淡出的品牌色晕染
    pub header_wash: Hsla,
    pub search_bg: Hsla,
    pub search_border: Hsla,
    /// 搜索框的聚焦光晕
    pub search_glow: Hsla,
    /// "记录中"状态徽标
    pub live_bg: Hsla,
    pub live_text: Hsla,
    pub live_dot: Hsla,
    /// 浮层（动作菜单）的底色，基本不透明
    pub popover: Hsla,
    /// 浮层打开时盖在弹窗上的遮罩
    pub scrim: Hsla,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                panel: c(0x1C1D22E0),
                surface: c(0xFFFFFF0A),
                hover: c(0xFFFFFF0D),
                selected: c(0x7C8CFF24),
                border: c(0xFFFFFF1A),
                divider: c(0xFFFFFF12),
                text: c(0xEDEDF2FF),
                text_muted: c(0xA3A5AFFF),
                text_faint: c(0x6E717BFF),
                accent: c(0x8B9BFFFF),
                accent_soft: c(0x8B9BFF2E),
                accent_text: c(0xC9D0FFFF),
                highlight_bg: c(0x8B9BFF40),
                kbd_bg: c(0xFFFFFF0F),
                kbd_border: c(0xFFFFFF1F),
                code_bg: c(0x00000033),
                header_wash: c(0x8B9BFF1F),
                search_bg: c(0xFFFFFF0F),
                search_border: c(0x8B9BFF66),
                search_glow: c(0x8B9BFF26),
                live_bg: c(0x34D3991F),
                live_text: c(0x6EE7B7FF),
                live_dot: c(0x34D399FF),
                popover: c(0x26272DFA),
                scrim: c(0x00000047),
            }
        } else {
            Self {
                dark,
                panel: c(0xF8F8FAE6),
                surface: c(0xFFFFFFB3),
                hover: c(0x0000000A),
                selected: c(0x4F6BED17),
                border: c(0x00000014),
                divider: c(0x0000000F),
                text: c(0x1B1C20FF),
                text_muted: c(0x5D5F68FF),
                text_faint: c(0x9497A1FF),
                accent: c(0x4F6BEDFF),
                accent_soft: c(0x4F6BED1F),
                accent_text: c(0x3550D6FF),
                highlight_bg: c(0x4F6BED33),
                kbd_bg: c(0xFFFFFFCC),
                kbd_border: c(0x0000001F),
                code_bg: c(0x0000000A),
                header_wash: c(0x4F6BED14),
                search_bg: c(0xFFFFFFF2),
                search_border: c(0x4F6BED59),
                search_glow: c(0x4F6BED1A),
                live_bg: c(0x10B98117),
                live_text: c(0x047857FF),
                live_dot: c(0x10B981FF),
                popover: c(0xFFFFFFFA),
                scrim: c(0x0F172A1F),
            }
        }
    }

    /// 每种内容类型的强调色（图标、徽标底色）
    pub fn kind_color(&self, kind: Kind) -> Hsla {
        let hex = match kind {
            Kind::Text => 0x7B8494FF,
            Kind::Link => 0x3B82F6FF,
            Kind::Code => 0x8B5CF6FF,
            Kind::Color => 0xEC4899FF,
            Kind::Image => 0x10B981FF,
            Kind::File => 0xF59E0BFF,
        };
        c(hex)
    }

    pub fn kind_soft(&self, kind: Kind) -> Hsla {
        let mut color = self.kind_color(kind);
        color.a = if self.dark { 0.22 } else { 0.14 };
        color
    }
}

/// 解析 `#RGB` / `#RRGGBB` / `#RRGGBBAA` 颜色字符串
pub fn parse_hex_color(s: &str) -> Option<Hsla> {
    parse_hex_rgba(s).map(c)
}

/// 把组件库（gpui-kit）的主色换成品牌色：开关、主按钮、焦点环等。
/// gpui-kit 切换深浅色时会重置主题颜色，所以每次切换主题后都要调用。
pub fn apply_brand(cx: &mut gpui_kit::App) {
    use gpui_kit::component::Theme;
    let dark = Theme::global(cx).is_dark();
    let p = Palette::new(dark);
    let shade = |delta: f32| {
        let mut c = p.accent;
        c.l = (c.l + delta).clamp(0.0, 1.0);
        c
    };
    let theme = Theme::global_mut(cx);
    theme.primary = if dark { shade(-0.12) } else { p.accent };
    theme.primary_hover = if dark { shade(-0.06) } else { shade(0.05) };
    theme.primary_active = if dark { shade(-0.16) } else { shade(-0.06) };
    theme.primary_foreground = gpui_kit::white();
    theme.ring = p.accent;
    // 组件实际读取的是由 colors 推导出的 tokens（开关、主按钮都用它），推导之后改 colors 不会同步，要一起改
    let (primary, hover, active) = (theme.primary, theme.primary_hover, theme.primary_active);
    let fg = theme.primary_foreground;
    theme.tokens.primary = primary.into();
    theme.tokens.primary_hover = hover.into();
    theme.tokens.primary_active = active.into();
    theme.tokens.primary_foreground = fg.into();
    theme.tokens.button_primary = primary.into();
    theme.tokens.button_primary_hover = hover.into();
    theme.tokens.button_primary_active = active.into();
    theme.tokens.button_primary_foreground = fg.into();
    theme.tokens.ring = p.accent.into();
}
