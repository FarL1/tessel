//! 安装界面：欢迎（可选安装位置）→ 安装中 → 完成 / 出错。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_core::i18n::tr;
use tessel_core::trf;

use crate::ui::{self, Closable, Colors, MIN_BUSY, VERSION};
use crate::{payload, system};

#[derive(Clone, PartialEq)]
enum Page {
    Welcome,
    Installing,
    Done,
    Failed(String),
}

/// 已经装过的版本和这个安装包的关系
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Fresh,
    Upgrade,
    Reinstall,
    Downgrade,
}

pub struct Setup {
    page: Page,
    mode: Mode,
    installed: Option<String>,
    running: bool,
    hotkey: String,
    /// 安装到哪里。已经装过时固定为原来的位置
    dir: PathBuf,
    dir_error: Option<String>,
    launch: bool,
    autostart: bool,
    /// 点了"完成并启动"，正在等 Tessel 的窗口出现
    launching: bool,
    /// 完成页上保存开机自启或启动 Tessel 出错
    finish_error: Option<String>,
    logo: Arc<Image>,
    focus: FocusHandle,
}

pub fn open(hotkey: String, cx: &mut App) {
    ui::open(
        tr("Tessel 拼贴 安装程序", "Tessel Setup"),
        (600., 560.),
        cx,
        move |cx| Setup::new(hotkey, cx),
    );
}

impl Setup {
    fn new(hotkey: String, cx: &mut Context<Self>) -> Self {
        let existing = system::existing_install();
        let installed = existing.as_deref().and_then(system::installed_version);
        let mode = match installed.as_deref() {
            None => Mode::Fresh,
            Some(v) if v == VERSION => Mode::Reinstall,
            Some(v) if is_newer(v, VERSION) => Mode::Downgrade,
            Some(_) => Mode::Upgrade,
        };
        let dir = existing.unwrap_or_else(system::default_install_dir);
        let dir_error = system::check_target(&dir).err();
        Self {
            page: Page::Welcome,
            mode,
            installed,
            running: system::tessel_running(),
            hotkey,
            dir,
            dir_error,
            launch: true,
            // 新装默认开机自启；更新时沿用原来的选择
            autostart: mode == Mode::Fresh || system::autostart_registered(),
            launching: false,
            finish_error: None,
            logo: ui::logo(),
            focus: cx.focus_handle(),
        }
    }

    /// 选择安装位置（只有新装时可以选；已经装过的话更新到原来的位置）
    fn choose_dir(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr("安装到这里", "Install here").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(picked) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let dir = system::install_target(&picked);
                this.dir_error = system::check_target(&dir).err();
                this.dir = dir;
                cx.notify();
            });
        })
        .detach();
    }

    fn reset_dir(&mut self, cx: &mut Context<Self>) {
        self.dir = system::default_install_dir();
        self.dir_error = system::check_target(&self.dir).err();
        cx.notify();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        if self.page == Page::Installing || self.dir_error.is_some() {
            return;
        }
        self.page = Page::Installing;
        cx.notify();
        let dir = self.dir.clone();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let source = payload::locate()?;
                    let setup = payload::extract(&source)?;
                    let result = system::run_velopack(&setup, &dir);
                    payload::cleanup(&source, &setup);
                    result?;
                    // 卸载程序装不上不影响使用："设置 → 应用"里仍然可以用 Velopack 自带的卸载
                    let _ = system::install_uninstaller(&dir);
                    Ok::<_, String>(())
                })
                .await;
            if let Some(rest) = MIN_BUSY.checked_sub(started.elapsed()) {
                cx.background_executor().timer(rest).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.page = match result {
                    Ok(()) => Page::Done,
                    Err(err) => Page::Failed(err),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 完成页：保存开机自启的选择；`launch` 为 true 时启动 Tessel，等它的窗口出现后再退出
    fn finish(&mut self, launch: bool, cx: &mut Context<Self>) {
        if self.launching {
            return;
        }
        // 出错时留在完成页上说明，不悄悄退出
        if let Err(e) = system::set_autostart(&self.dir, self.autostart) {
            self.finish_error = Some(trf!(
                "没能设置开机自启：{e}。可以取消勾选后再点完成，以后在 Tessel 的设置里打开。",
                "Couldn't set up start with Windows: {e}. Untick it and finish; you can turn it on later in Tessel's settings."
            ));
            cx.notify();
            return;
        }
        if !launch {
            cx.quit();
            return;
        }
        self.launching = true;
        self.finish_error = None;
        cx.notify();
        let dir = self.dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { system::launch_app(&dir) })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.quit(),
                Err(e) => {
                    this.launching = false;
                    this.finish_error = Some(trf!(
                        "没能启动 Tessel：{e}。可以从开始菜单打开它。",
                        "Couldn't start Tessel: {e}. You can open it from the Start menu."
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn primary(&mut self, cx: &mut Context<Self>) {
        match self.page {
            Page::Welcome | Page::Failed(_) => self.install(cx),
            Page::Done => self.finish(self.launch, cx),
            Page::Installing => {}
        }
    }
}

impl Closable for Setup {
    fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 关闭按钮 / Esc / Alt+F4
    fn close(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        match self.page {
            // 安装过程中不能中途退出
            Page::Installing => {}
            // 关闭 = 不启动（开机自启按勾选保存）
            // 已经显示过出错提示的话，再关就直接退出
            Page::Done if self.finish_error.is_some() && !self.launching => cx.quit(),
            Page::Done => self.finish(false, cx),
            Page::Welcome | Page::Failed(_) => cx.quit(),
        }
    }
}

/// "0.3.0" 是否比 "0.2.1" 新（只比较数字部分）
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    }
    parts(candidate) > parts(current)
}

/// 一行特性介绍：图标 + 标题 + 说明
fn feature(icon: impl Into<Icon>, title: &str, desc: String, p: &Colors) -> Div {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .rounded(px(8.))
                .bg(p.accent_soft)
                .text_color(p.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(icon.into().small()),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.text)
                        .child(title.to_string()),
                )
                .child(div().text_xs().text_color(p.muted).child(desc)),
        )
}

impl Setup {
    fn render_welcome(&self, p: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let keys = self.hotkey.replace(' ', "");
        let hero = h_flex()
            .gap_4()
            .child(img(self.logo.clone()).size(px(64.)).flex_none())
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_end()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(28.))
                                    .line_height(px(32.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child("Tessel"),
                            )
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .line_height(px(28.))
                                    .text_color(p.accent)
                                    .child("拼贴"),
                            )
                            .child(
                                div()
                                    .mb(px(5.))
                                    .px_1p5()
                                    .rounded(px(4.))
                                    .bg(p.accent_soft)
                                    .text_xs()
                                    .text_color(p.accent_text)
                                    .child(format!("v{VERSION}")),
                            ),
                    )
                    .child(div().text_sm().text_color(p.muted).child(tr(
                        "Windows 剪贴板管理器 · 每一次复制，都拼进你的工作里",
                        "A keyboard-first clipboard manager for Windows",
                    ))),
            );

        let features = v_flex()
            .gap_2p5()
            .child(feature(
                Lucide::ClipboardClock,
                tr("复制过的都在这里", "Everything you copied, in one place"),
                trf!(
                    "文字、图片、文件、链接自动记录，按 {keys} 随时找回",
                    "Text, images, files and links are saved. Press {keys} to bring them back"
                ),
                p,
            ))
            .child(feature(
                IconName::Search,
                tr("找得快", "Find it fast"),
                tr(
                    "支持拼音首字母、图片里的文字，还有收藏、片段和粘贴队列",
                    "Search by pinyin or text inside images; pins, snippets and a paste queue",
                )
                .into(),
                p,
            ))
            .child(feature(
                Lucide::ShieldCheck,
                tr("数据只在你的电脑上", "Your data stays on this PC"),
                tr(
                    "开源免费、不用注册账号；密码管理器里复制的内容不会被记录",
                    "Open source and free, no account; copies from password managers are skipped",
                )
                .into(),
                p,
            ));

        let locked = self.mode != Mode::Fresh;
        let custom = self.dir != system::default_install_dir();
        let location = v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .bg(p.surface)
            .border_1()
            .border_color(if self.dir_error.is_some() {
                p.danger
            } else {
                p.border
            })
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(Icon::new(IconName::FolderOpen).xsmall().text_color(p.muted))
                    .child(
                        div()
                            .flex_none()
                            .text_color(p.muted)
                            .child(tr("安装位置", "Install location")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(p.text)
                            .child(self.dir.display().to_string()),
                    )
                    .when(!locked && custom, |d| {
                        d.child(
                            Button::new("reset-dir")
                                .ghost()
                                .xsmall()
                                .label(tr("恢复默认", "Default"))
                                .on_click(cx.listener(|this, _, _, cx| this.reset_dir(cx))),
                        )
                    })
                    .when(!locked, |d| {
                        d.child(
                            Button::new("choose-dir")
                                .outline()
                                .xsmall()
                                .label(tr("更改…", "Change…"))
                                .on_click(cx.listener(|this, _, _, cx| this.choose_dir(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .pl(px(20.))
                    .text_xs()
                    .text_color(p.faint)
                    .child(if locked {
                        tr(
                            "已经安装在这里，会装到同一个位置。想换位置请先卸载。",
                            "Tessel is already installed here and will stay here. Uninstall first to move it.",
                        )
                    } else {
                        tr(
                            "只为当前用户安装，不需要管理员权限；会创建桌面和开始菜单快捷方式",
                            "Current user only, no admin rights needed. Adds Desktop and Start menu shortcuts",
                        )
                    }),
            );

        let installed = self.installed.clone().unwrap_or_default();
        let notice = match self.mode {
            Mode::Fresh => None,
            Mode::Upgrade => Some(trf!(
                "已安装 {installed}，将更新到 {VERSION}，剪贴板历史和设置都会保留。",
                "Version {installed} is installed and will be updated to {VERSION}. Your history and settings are kept."
            )),
            Mode::Reinstall => Some(trf!(
                "已经安装了 {installed}，可以重新安装来修复，剪贴板历史和设置都会保留。",
                "Version {installed} is already installed. Reinstall to repair it; your history and settings are kept."
            )),
            Mode::Downgrade => Some(trf!(
                "已安装更新的版本 {installed}，继续会换成较旧的 {VERSION}，剪贴板历史和设置都会保留。",
                "A newer version ({installed}) is installed. Continuing replaces it with {VERSION}. Your history and settings are kept."
            )),
        };
        let notice = match (notice, self.running) {
            (Some(n), true) => Some(trf!(
                "{n}Tessel 正在运行，安装时会先把它关闭。",
                "{n} Tessel is running and will be closed first."
            )),
            (None, true) => Some(
                tr(
                    "Tessel 正在运行，安装时会先把它关闭。",
                    "Tessel is running and will be closed first.",
                )
                .into(),
            ),
            (n, false) => n,
        };

        let action = match self.mode {
            Mode::Fresh => tr("安装", "Install"),
            Mode::Upgrade => tr("更新", "Update"),
            Mode::Reinstall | Mode::Downgrade => tr("重新安装", "Reinstall"),
        };

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .px_8()
                    .pt_2()
                    .gap_4()
                    .child(hero)
                    .child(features)
                    .child(location)
                    .when_some(self.dir_error.clone(), |d, e| {
                        d.child(ui::notice(e, true, p))
                    })
                    .when_some(notice, |d, n| d.child(ui::notice(n, false, p))),
            )
            .child(ui::footer(
                div().text_xs().text_color(p.faint).child(tr(
                    "开源软件 · MIT / Apache-2.0",
                    "Open source · MIT / Apache-2.0",
                )),
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("cancel")
                            .ghost()
                            .label(tr("取消", "Cancel"))
                            .on_click(cx.listener(|_, _, _, cx| cx.quit())),
                    )
                    .child(
                        Button::new("install")
                            .primary()
                            .label(action)
                            .disabled(self.dir_error.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    ),
                p,
            ))
    }

    fn render_done(&self, p: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let keys: Vec<String> = self
            .hotkey
            .split('+')
            .map(|k| k.trim().to_string())
            .collect();
        let (title, subtitle) = match self.mode {
            Mode::Upgrade => (
                tr("更新完成", "Update complete"),
                trf!(
                    "Tessel 已经更新到 {VERSION}。",
                    "Tessel has been updated to {VERSION}."
                ),
            ),
            _ => (
                tr("安装完成", "Setup complete"),
                trf!(
                    "Tessel {VERSION} 已经安装到这台电脑。",
                    "Tessel {VERSION} is installed on this PC."
                ),
            ),
        };

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
                                    .bg(p.success)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::Check).xsmall().text_color(white())),
                            )
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child(title),
                            ),
                    )
                    .child(div().text_sm().text_color(p.muted).child(subtitle)),
            );

        let tip = v_flex()
            .gap_2()
            .p_4()
            .rounded(px(10.))
            .bg(p.surface)
            .border_1()
            .border_color(p.border)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .text_sm()
                    .text_color(p.text)
                    .child(Icon::new(Lucide::Keyboard).small().text_color(p.accent))
                    .child(tr("在任何程序里按", "Press"))
                    .children(keys.into_iter().map(|k| ui::keycap(k, p)))
                    .child(tr(
                        "打开剪贴板历史。",
                        "anywhere to open your clipboard history.",
                    )),
            )
            .child(div().text_xs().text_color(p.faint).child(tr(
                "Tessel 平时待在任务栏右下角的托盘里，右键托盘图标可以打开设置或退出。以后可以在\"设置 → 应用\"里卸载。",
                "Tessel lives in the system tray; right-click its icon for settings or to quit. Uninstall it any time from Settings → Apps.",
            )));

        let options = v_flex()
            .gap_3()
            .child(
                Checkbox::new("launch")
                    .checked(self.launch)
                    .label(tr("现在启动 Tessel", "Start Tessel now"))
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.launch = *checked;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("autostart")
                    .checked(self.autostart)
                    .label(tr("开机时自动启动", "Start with Windows"))
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.autostart = *checked;
                        cx.notify();
                    })),
            );

        let label = if self.launching {
            tr("正在启动…", "Starting…")
        } else if self.launch {
            tr("完成并启动", "Finish and start")
        } else {
            tr("完成", "Finish")
        };
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .px_8()
                    .pt_6()
                    .gap_6()
                    .child(heading)
                    .child(tip)
                    .child(options)
                    .when_some(self.finish_error.clone(), |d, e| {
                        d.child(ui::notice(e, true, p))
                    }),
            )
            .child(ui::footer(
                div(),
                Button::new("finish")
                    .primary()
                    .label(label)
                    .loading(self.launching)
                    .on_click(cx.listener(|this, _, _, cx| this.finish(this.launch, cx))),
                p,
            ))
    }

    fn render_failed(&self, err: &str, p: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let log = system::velopack_log();
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .px_8()
                    .pt_6()
                    .gap_5()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(40.))
                                    .rounded_full()
                                    .bg(p.danger_soft)
                                    .text_color(p.danger)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::TriangleAlert).small()),
                            )
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.text)
                                            .child(tr("安装没有完成", "Setup didn't finish")),
                                    )
                                    .child(div().text_sm().text_color(p.muted).child(
                                        if self.mode == Mode::Fresh {
                                            tr("Tessel 还没有装上。", "Tessel wasn't installed.")
                                        } else {
                                            tr(
                                                "原来的 Tessel 可能已经被关闭，重试成功后就能继续使用。",
                                                "Your existing Tessel may have been closed. It will work again once setup succeeds.",
                                            )
                                        },
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .p_3()
                            .rounded(px(8.))
                            .bg(p.surface)
                            .border_1()
                            .border_color(p.border)
                            .text_sm()
                            .text_color(p.text)
                            .child(err.to_string()),
                    )
                    .child(div().text_xs().text_color(p.faint).child(trf!(
                        "可以先重试一次；还是不行的话，请把安装日志附在问题反馈里：{}",
                        "Try again first. If it keeps failing, please attach the setup log to your report: {}",
                        log.display()
                    ))),
            )
            .child(ui::footer(
                Button::new("log")
                    .ghost()
                    .small()
                    .label(tr("查看日志", "Open log"))
                    .on_click(move |_, _, _| system::open_path(&log)),
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("close")
                            .ghost()
                            .label(tr("关闭", "Close"))
                            .on_click(cx.listener(|_, _, _, cx| cx.quit())),
                    )
                    .child(
                        Button::new("retry")
                            .primary()
                            .label(tr("重试", "Try again"))
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    ),
                p,
            ))
    }
}

impl Render for Setup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Colors::new(cx.theme().is_dark());
        let page = self.page.clone();
        let (key, body) = match &page {
            Page::Welcome => ("welcome", self.render_welcome(&p, cx).into_any_element()),
            Page::Installing => (
                "installing",
                ui::busy_page(
                    &self.logo,
                    match self.mode {
                        Mode::Upgrade => tr("正在更新 Tessel…", "Updating Tessel…"),
                        _ => tr("正在安装 Tessel…", "Installing Tessel…"),
                    },
                    tr(
                        "通常只需要几秒钟，请不要关闭这个窗口",
                        "This usually takes a few seconds. Please keep this window open",
                    ),
                    &p,
                )
                .into_any_element(),
            ),
            Page::Done => ("done", self.render_done(&p, cx).into_any_element()),
            Page::Failed(err) => ("failed", self.render_failed(err, &p, cx).into_any_element()),
        };
        let bar = ui::title_bar(
            tr("Tessel 拼贴 安装程序", "Tessel Setup"),
            &self.logo,
            page != Page::Installing,
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
