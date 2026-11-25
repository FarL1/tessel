//! 安装完成页：安装程序装好后第一次启动时显示（Velopack 的 first-run），
//! 让用户自己决定现在要不要启动 Tessel、要不要开机自启，而不是装完直接弹出来。
//!
//! 测试时可以用环境变量 `TESSEL_FIRST_RUN=1` 强制显示。

use std::sync::Arc;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, TitleBar};
use gpui_kit::*;
use tessel_core::i18n::tr;
use tessel_core::trf;

use crate::design::{self, Palette};
use crate::{AppCommand, log_line};

pub struct Welcome {
    tx: async_channel::Sender<AppCommand>,
    hotkey: String,
    launch: bool,
    autostart: bool,
    finished: bool,
    logo: Arc<Image>,
}

impl Welcome {
    /// `launch`：点"完成"时按勾选；点关闭按钮时一律不启动
    fn finish(&mut self, launch: bool, window: &mut Window) {
        if self.finished {
            return;
        }
        self.finished = true;
        let _ = self.tx.try_send(AppCommand::FinishSetup {
            launch,
            autostart: self.autostart,
        });
        window.remove_window();
    }
}

fn keycap(label: String, p: &Palette) -> Div {
    div()
        .h(px(22.))
        .min_w(px(22.))
        .px_1p5()
        .rounded(px(5.))
        .bg(p.kbd_bg)
        .border_1()
        .border_color(p.kbd_border)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.text)
        .flex()
        .items_center()
        .justify_center()
        .child(label)
}

impl Render for Welcome {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(cx.theme().is_dark());
        let (launch, autostart) = (self.launch, self.autostart);
        let keys: Vec<String> = self
            .hotkey
            .split('+')
            .map(|k| k.trim().to_string())
            .collect();

        let heading = h_flex()
            .gap_4()
            .child(img(self.logo.clone()).size(px(64.)).flex_none())
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(22.))
                                    .rounded_full()
                                    .bg(rgb(0x22C55E))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::Check)
                                            .xsmall()
                                            .text_color(rgb(0xFFFFFF)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child(tr("安装完成", "Setup complete")),
                            ),
                    )
                    .child(div().text_sm().text_color(p.text_muted).child(trf!(
                        "Tessel {} 已经安装到这台电脑。",
                        "Tessel {} is installed on this PC.",
                        env!("CARGO_PKG_VERSION")
                    ))),
            );

        // 怎么用：一句话 + 热键键帽
        let tip = v_flex()
            .gap_2()
            .p_4()
            .rounded(design::RADIUS_LG)
            .bg(p.surface)
            .border_1()
            .border_color(p.border)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .text_sm()
                    .text_color(p.text)
                    .child(tr("在任何程序里按", "Press"))
                    .children(keys.into_iter().map(|k| keycap(k, &p)))
                    .child(tr("打开剪贴板历史。", "anywhere to open your clipboard history.")),
            )
            .child(div().text_xs().text_color(p.text_faint).child(tr(
                "Tessel 平时待在任务栏右下角的托盘里，复制的内容会自动记下来，数据只保存在这台电脑上。",
                "Tessel lives in the system tray and remembers what you copy. Your data stays on this PC.",
            )));

        let options = v_flex()
            .gap_3()
            .child(
                Checkbox::new("launch")
                    .checked(launch)
                    .label(tr("现在启动 Tessel", "Start Tessel now"))
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.launch = *checked;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("autostart")
                    .checked(autostart)
                    .label(tr("开机时自动启动", "Start with Windows"))
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.autostart = *checked;
                        cx.notify();
                    })),
            );

        v_flex()
            .size_full()
            .bg(p.panel)
            .text_color(p.text)
            // 自己画标题栏：组件库的 TitleBar 在 Windows 上总会显示最大化按钮，而这个窗口不能调大小
            .child(
                h_flex()
                    .h(px(36.))
                    .flex_none()
                    .pl_3()
                    .child(
                        h_flex()
                            .id("drag")
                            .flex_1()
                            .h_full()
                            .gap_2()
                            .on_mouse_down(MouseButton::Left, |_, window, _| {
                                window.start_window_move()
                            })
                            .child(img(self.logo.clone()).size(px(16.)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .child(tr("Tessel 拼贴 安装", "Tessel Setup")),
                            ),
                    )
                    .child(
                        div()
                            .id("close")
                            .w(px(46.))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(p.text_muted)
                            .hover(|s| s.bg(rgb(0xC42B1C)).text_color(rgb(0xFFFFFF)))
                            .child(Icon::new(IconName::Close).small())
                            .on_click(cx.listener(|this, _, window, _| this.finish(false, window))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .px_8()
                    .pt_6()
                    .pb_6()
                    .gap_6()
                    .child(heading)
                    .child(tip)
                    .child(options)
                    .child(div().flex_1())
                    .child(
                        h_flex().justify_end().child(
                            Button::new("finish")
                                .primary()
                                .label(if launch {
                                    tr("完成并启动", "Finish and start")
                                } else {
                                    tr("完成", "Finish")
                                })
                                .on_click(cx.listener(|this, _, window, _| {
                                    this.finish(this.launch, window)
                                })),
                        ),
                    ),
            )
            .with_animation(
                "welcome-in",
                Animation::new(std::time::Duration::from_millis(220)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t),
            )
    }
}

/// 打开安装完成页（放在鼠标所在屏幕的中间）
pub fn open(tx: async_channel::Sender<AppCommand>, hotkey: String, cx: &mut App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let bounds = Bounds::centered(None, size(px(520.), px(440.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitleBar::title_bar_options()),
        kind: WindowKind::Normal,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        ..Default::default()
    };
    let close_tx = tx.clone();
    let mut hwnd = None;
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title(tr("Tessel 拼贴 安装", "Tessel Setup"));
        if let Ok(RawWindowHandle::Win32(h)) = window.window_handle().map(|h| h.as_raw()) {
            let raw = h.hwnd.get();
            crate::win::hide(raw);
            crate::win::place_over(raw, None);
            hwnd = Some(raw);
        }
        let view = cx.new(|_| Welcome {
            tx,
            hotkey,
            launch: true,
            autostart: true,
            finished: false,
            logo: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../../../assets/brand/png/tessel-128.png").to_vec(),
            )),
        });
        // Alt+F4 / 任务栏关闭：和关闭按钮一样，不启动（开机自启按勾选保存）
        let weak = view.downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(view) = weak.upgrade() {
                view.update(cx, |this, _| {
                    if !this.finished {
                        this.finished = true;
                        let _ = close_tx.try_send(AppCommand::FinishSetup {
                            launch: false,
                            autostart: this.autostart,
                        });
                    }
                });
            }
            let _ = window;
            true
        });
        view
    });
    match opened {
        Ok(_) => {
            if let Some(raw) = hwnd {
                crate::win::restore_and_focus(raw);
            }
        }
        Err(err) => log_line(format!("[error] 打开安装完成页失败：{err}")),
    }
}
