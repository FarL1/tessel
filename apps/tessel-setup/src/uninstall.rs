//! 卸载界面：确认（可选同时删除数据）→ 卸载中 → 完成 / 出错。
//!
//! 安装时放进安装目录的 `Uninstall Tessel.exe` 就是本程序，"设置 → 应用"里的卸载入口指向它，
//! 以 `--uninstall` 启动。真正的卸载交给 Velopack 的 `Update.exe --uninstall --silent`。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_core::i18n::tr;
use tessel_core::trf;

use crate::system;
use crate::ui::{self, Closable, Colors, MIN_BUSY};

#[derive(Clone, PartialEq)]
enum Page {
    Confirm,
    Removing,
    Done,
    Failed(String),
    NotInstalled,
}

pub struct Uninstall {
    page: Page,
    dir: Option<PathBuf>,
    version: Option<String>,
    running: bool,
    delete_data: bool,
    /// 程序已经卸载掉了（只剩删除数据出错时，重试只需要再删数据）
    app_removed: bool,
    logo: Arc<Image>,
    focus: FocusHandle,
}

pub fn open(cx: &mut App) {
    ui::open(
        tr("卸载 Tessel 拼贴", "Uninstall Tessel"),
        (560., 440.),
        cx,
        Uninstall::new,
    );
}

impl Uninstall {
    fn new(cx: &mut Context<Self>) -> Self {
        let dir = system::existing_install();
        Self {
            page: if dir.is_some() {
                Page::Confirm
            } else {
                Page::NotInstalled
            },
            version: dir.as_deref().and_then(system::installed_version),
            dir,
            running: system::tessel_running(),
            delete_data: false,
            app_removed: false,
            logo: ui::logo(),
            focus: cx.focus_handle(),
        }
    }

    fn uninstall(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        if self.page == Page::Removing {
            return;
        }
        self.page = Page::Removing;
        cx.notify();
        let delete_data = self.delete_data;
        let app_removed = self.app_removed;
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let (removed, result) = cx
                .background_executor()
                .spawn(async move {
                    if !app_removed && let Err(e) = system::run_uninstall(&dir) {
                        return (false, Err(e));
                    }
                    let result = if delete_data {
                        system::delete_data()
                    } else {
                        Ok(())
                    };
                    (true, result)
                })
                .await;
            if let Some(rest) = MIN_BUSY.checked_sub(started.elapsed()) {
                cx.background_executor().timer(rest).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.app_removed = removed;
                this.page = match result {
                    Ok(()) => Page::Done,
                    Err(err) => Page::Failed(err),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn quit(cx: &mut App) {
        system::schedule_self_delete();
        cx.quit();
    }

    fn primary(&mut self, cx: &mut Context<Self>) {
        match self.page {
            // 卸载是删除操作：确认页上不响应回车，要用鼠标点"卸载"
            Page::Confirm => {}
            Page::Failed(_) => self.uninstall(cx),
            Page::Done | Page::NotInstalled => Self::quit(cx),
            Page::Removing => {}
        }
    }

    fn render_confirm(&self, p: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let dir = self.dir.clone().unwrap_or_default();
        let version = self.version.clone().unwrap_or_default();
        let data = system::data_dir();
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .px_8()
                    .pt_4()
                    .gap_5()
                    .child(
                        h_flex()
                            .gap_4()
                            .child(img(self.logo.clone()).size(px(56.)).flex_none())
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.text)
                                            .child(tr("卸载 Tessel", "Uninstall Tessel")),
                                    )
                                    .child(div().text_sm().text_color(p.muted).child(trf!(
                                        "将从这台电脑上删除 Tessel {version} 和它的快捷方式。",
                                        "Tessel {version} and its shortcuts will be removed from this PC."
                                    ))),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .bg(p.surface)
                            .border_1()
                            .border_color(p.border)
                            .text_xs()
                            .child(Icon::new(IconName::FolderOpen).xsmall().text_color(p.muted))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(p.muted)
                                    .child(tr("安装位置", "Installed in")),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(p.text)
                                    .child(dir.display().to_string()),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(
                                Checkbox::new("delete-data")
                                    .checked(self.delete_data)
                                    .label(tr(
                                        "同时删除剪贴板历史和设置",
                                        "Also delete my clipboard history and settings",
                                    ))
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.delete_data = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .pl(px(26.))
                                    .text_xs()
                                    .text_color(if self.delete_data {
                                        p.danger
                                    } else {
                                        p.faint
                                    })
                                    .child(if self.delete_data {
                                        trf!(
                                            "会删除 {}，删除后无法恢复。",
                                            "{} will be deleted. This can't be undone.",
                                            data.display()
                                        )
                                    } else {
                                        trf!(
                                            "不勾选的话会保留在 {}，以后重新安装可以继续使用。",
                                            "If unchecked they stay in {}, ready if you reinstall.",
                                            data.display()
                                        )
                                    }),
                            ),
                    )
                    .when(self.running, |d| {
                        d.child(ui::notice(
                            tr(
                                "Tessel 正在运行，卸载时会先把它关闭。",
                                "Tessel is running and will be closed first.",
                            )
                            .into(),
                            false,
                            p,
                        ))
                    }),
            )
            .child(ui::footer(
                div(),
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("cancel")
                            .ghost()
                            .label(tr("取消", "Cancel"))
                            .on_click(cx.listener(|_, _, _, cx| Self::quit(cx))),
                    )
                    .child(
                        Button::new("uninstall")
                            .danger()
                            .icon(Icon::new(Lucide::Trash))
                            .label(tr("卸载", "Uninstall"))
                            .on_click(cx.listener(|this, _, _, cx| this.uninstall(cx))),
                    ),
                p,
            ))
    }

    fn render_result(
        &self,
        ok: bool,
        title: &'static str,
        body: String,
        p: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (bg, fg, icon) = if ok {
            (p.success, white(), IconName::Check)
        } else {
            (p.danger_soft, p.danger, IconName::TriangleAlert)
        };
        let failed = matches!(self.page, Page::Failed(_));
        let log = system::velopack_log();
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .px_8()
                    .pt_6()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(36.))
                                    .rounded_full()
                                    .bg(bg)
                                    .text_color(fg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(icon).small()),
                            )
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child(title),
                            ),
                    )
                    .child(div().text_sm().text_color(p.muted).child(body)),
            )
            .child(ui::footer(
                if failed {
                    Button::new("log")
                        .ghost()
                        .small()
                        .label(tr("查看日志", "Open log"))
                        .on_click(move |_, _, _| system::open_path(&log))
                        .into_any_element()
                } else {
                    div().into_any_element()
                },
                h_flex()
                    .gap_2()
                    .when(failed, |d| {
                        d.child(
                            Button::new("retry")
                                .ghost()
                                .label(tr("重试", "Try again"))
                                .on_click(cx.listener(|this, _, _, cx| this.uninstall(cx))),
                        )
                    })
                    .child(
                        Button::new("close")
                            .primary()
                            .label(tr("关闭", "Close"))
                            .on_click(cx.listener(|_, _, _, cx| Self::quit(cx))),
                    ),
                p,
            ))
    }
}

impl Closable for Uninstall {
    fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn close(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Removing {
            Self::quit(cx);
        }
    }
}

impl Render for Uninstall {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Colors::new(cx.theme().is_dark());
        let page = self.page.clone();
        let data = system::data_dir();
        let (key, body) = match &page {
            Page::Confirm => ("confirm", self.render_confirm(&p, cx).into_any_element()),
            Page::Removing => (
                "removing",
                ui::busy_page(
                    &self.logo,
                    tr("正在卸载 Tessel…", "Uninstalling Tessel…"),
                    tr("通常只需要几秒钟", "This usually takes a few seconds"),
                    &p,
                )
                .into_any_element(),
            ),
            Page::Done => (
                "done",
                self.render_result(
                    true,
                    tr("Tessel 已卸载", "Tessel has been uninstalled"),
                    if self.delete_data {
                        tr(
                            "程序、快捷方式、剪贴板历史和设置都已删除。感谢使用 Tessel！",
                            "The app, its shortcuts, your history and settings have been removed. Thanks for using Tessel!",
                        )
                        .into()
                    } else {
                        trf!(
                            "剪贴板历史和设置保留在 {}，重新安装后会继续使用；不需要的话可以手动删除。感谢使用 Tessel！",
                            "Your history and settings are kept in {} for when you reinstall; delete the folder if you don't need them. Thanks for using Tessel!",
                            data.display()
                        )
                    },
                    &p,
                    cx,
                )
                .into_any_element(),
            ),
            Page::Failed(err) => (
                "failed",
                self.render_result(
                    false,
                    if self.app_removed {
                        tr("数据没有删除完", "Your data wasn't fully deleted")
                    } else {
                        tr("卸载没有完成", "Uninstall didn't finish")
                    },
                    err.clone(),
                    &p,
                    cx,
                )
                .into_any_element(),
            ),
            Page::NotInstalled => (
                "missing",
                self.render_result(
                    false,
                    tr("没有找到 Tessel", "Tessel isn't installed"),
                    tr(
                        "这台电脑上没有找到已安装的 Tessel，可能已经卸载过了。",
                        "No installed copy of Tessel was found on this PC. It may already be uninstalled.",
                    )
                    .into(),
                    &p,
                    cx,
                )
                .into_any_element(),
            ),
        };
        let bar = ui::title_bar(
            tr("卸载 Tessel 拼贴", "Uninstall Tessel"),
            &self.logo,
            page != Page::Removing,
            cx.listener(|this, _, window, cx| this.close(window, cx)),
            &p,
        );
        ui::frame(key, bar, body, &p)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => this.primary(cx),
                    "escape" => this.close(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
    }
}
