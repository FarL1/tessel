//! 设置窗口。
//!
//! 界面读写的就是 `config.toml`：每次修改立即写回文件（保留注释），再走和"手动编辑文件"
//! 完全相同的热加载流程。所以界面修改和文本编辑永远一致，也只有一处校验逻辑。

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, TitleBar};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_core::{Config, ThemePref};

use crate::design::{self, Palette};
use crate::{AppCommand, log_line};

/// 历史条数的可选值
const KEEP_OPTIONS: [usize; 4] = [1_000, 5_000, 20_000, 100_000];

pub struct SettingsView {
    dir: PathBuf,
    tx: async_channel::Sender<AppCommand>,
    config: Config,
    autostart: bool,
    /// 正在录制新热键
    recording: bool,
    recorder_focus: FocusHandle,
    add_app: Entity<InputState>,
    /// 配置未生效的原因（文件有错、热键被占用等），随下一次重载结果更新
    error: Option<SharedString>,
    /// 一次性提示（例如"新热键被占用，已恢复为旧热键"），不会被配置重载清掉，下次录制时清除
    notice: Option<SharedString>,
    storage: String,
    logo: Arc<Image>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(
        dir: PathBuf,
        tx: async_channel::Sender<AppCommand>,
        config: Config,
        autostart: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let add_app =
            cx.new(|cx| InputState::new(window, cx).placeholder("程序文件名，例如 KeePassXC.exe"));
        let recorder_focus = cx.focus_handle();
        let subscriptions = vec![
            cx.subscribe_in(&add_app, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_ignored_app(window, cx);
                }
            }),
            // 录制时点到别处、切到别的窗口：取消录制
            cx.on_blur(&recorder_focus, window, |this, _, cx| {
                this.stop_recording(cx)
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.stop_recording(cx);
                }
            }),
            // 录制中直接关掉窗口：视图销毁时也要恢复全局热键
            cx.on_release(|this, _| {
                if this.recording {
                    let _ = this.tx.try_send(AppCommand::HotkeyRecording(false));
                }
            }),
        ];
        let storage = storage_summary(&dir);
        Self {
            dir,
            tx,
            config,
            autostart,
            recording: false,
            recorder_focus,
            add_app,
            error: None,
            notice: None,
            storage,
            logo: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../../../assets/brand/png/tessel-64.png").to_vec(),
            )),
            _subscriptions: subscriptions,
        }
    }

    /// 修改配置：以文件里的最新内容为准，只改这一项，写回文件，并立刻通知主线程重新加载。
    ///
    /// 不能直接保存 `self.config`：它可能是过期的（刚在编辑器里改过文件，监视器还没触发），
    /// 也可能是文件出错时的默认值，直接保存会覆盖用户的设置。
    fn change(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Config)) {
        let mut config = match Config::load_or_create(&self.dir) {
            Ok(config) => config,
            Err(err) => {
                self.error = Some(format!("{err}。请先修正 config.toml，再在这里修改。").into());
                cx.notify();
                return;
            }
        };
        f(&mut config);
        match config.save(&self.dir) {
            Ok(()) => {
                // 保存成功后才更新界面，避免界面和实际生效的设置不一致
                self.config = config;
                let _ = self.tx.try_send(AppCommand::ConfigChanged);
            }
            Err(err) => {
                log_line(format!("[settings] {err}"));
                self.error = Some(err.into());
            }
        }
        cx.notify();
    }

    pub fn set_notice(&mut self, notice: String, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        cx.notify();
    }

    /// 主线程重新加载配置之后调用（无论修改来自界面还是文本编辑器）
    /// `warning`：配置已生效，但有一部分没成功（比如热键被占用）
    pub fn on_config_result(
        &mut self,
        result: Result<Config, String>,
        warning: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(config) => {
                self.config = config;
                self.error = warning.map(Into::into);
            }
            Err(err) => self.error = Some(err.into()),
        }
        cx.notify();
    }

    pub fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        self.storage = storage_summary(&self.dir);
        cx.notify();
    }

    pub fn set_autostart_state(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.autostart = enabled;
        cx.notify();
    }

    // ── 热键录制 ──

    fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording {
            return;
        }
        self.recording = true;
        self.notice = None;
        self.recorder_focus.focus(window, cx);
        // 暂时注销全局热键，否则按下当前热键会被系统截走，录不到
        let _ = self.tx.try_send(AppCommand::HotkeyRecording(true));
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        self.recording = false;
        let _ = self.tx.try_send(AppCommand::HotkeyRecording(false));
        cx.notify();
    }

    fn on_record_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        cx.stop_propagation();
        let ks = &event.keystroke;
        if ks.key == "escape" {
            self.stop_recording(cx);
            return;
        }
        let Some(key) = hotkey_token(&ks.key) else {
            self.error = Some(format!("不支持「{}」这个键，请换一个", ks.key).into());
            cx.notify();
            return;
        };
        let m = &ks.modifiers;
        let is_fn_key = key.starts_with('F') && key.len() > 1;
        if !(m.control || m.alt || m.shift || m.platform) && !is_fn_key {
            self.error = Some("热键需要包含 Ctrl / Alt / Shift / Win 中的至少一个".into());
            cx.notify();
            return;
        }
        let mut parts = Vec::new();
        if m.control {
            parts.push("Ctrl".to_string());
        }
        if m.alt {
            parts.push("Alt".to_string());
        }
        if m.shift {
            parts.push("Shift".to_string());
        }
        if m.platform {
            parts.push("Win".to_string());
        }
        parts.push(key);
        let spec = parts.join("+");
        if let Err(err) = crate::parse_hotkey(&spec) {
            self.error = Some(err.into());
            cx.notify();
            return;
        }
        // 先发 ConfigChanged 再发恢复热键：主线程按顺序处理，会直接注册新热键
        self.change(cx, |c| c.hotkey = spec);
        self.stop_recording(cx);
    }

    // ── 忽略的应用 ──

    fn add_ignored_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.add_app.read(cx).value().trim().to_string();
        if raw.is_empty() {
            return;
        }
        // 允许粘贴完整路径，只保留文件名；没写扩展名时补上 .exe
        let mut name = tessel_core::model::file_name(&raw)
            .trim_matches('"')
            .to_string();
        if !name.to_ascii_lowercase().ends_with(".exe") {
            name.push_str(".exe");
        }
        if !self.config.is_ignored(&name) {
            self.change(cx, |c| c.ignored_apps.push(name));
        }
        self.add_app
            .update(cx, |state, cx| state.set_value("", window, cx));
    }

    fn remove_ignored_app(&mut self, name: String, cx: &mut Context<Self>) {
        // 按名字删除：文件可能在外部被改过，序号不可靠
        self.change(cx, |c| c.ignored_apps.retain(|app| *app != name));
    }
}

/// GPUI 的按键名 → global-hotkey 的按键名
fn hotkey_token(key: &str) -> Option<String> {
    let named = match key {
        "space" => "Space",
        "`" => "Backquote",
        "," => "Comma",
        "." => "Period",
        "insert" => "Insert",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.into());
    }
    if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(key.to_ascii_uppercase());
    }
    if let Some(n) = key.strip_prefix('f')
        && n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n))
    {
        return Some(key.to_ascii_uppercase());
    }
    None
}

fn dir_size(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

fn storage_summary(dir: &std::path::Path) -> String {
    let bytes = dir_size(dir);
    let size = if bytes < 1_048_576 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    };
    let count = tessel_core::Store::open(dir)
        .and_then(|s| s.counts())
        .map(|c| c.total)
        .unwrap_or(0);
    format!("{count} 条记录 · 占用 {size}")
}

// ───────────────────────────── 渲染 ─────────────────────────────

fn card(title: &'static str, p: &Palette) -> Div {
    v_flex().gap_2().child(
        div()
            .px_1()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.text_muted)
            .child(title),
    )
}

fn card_body(p: &Palette) -> Div {
    v_flex()
        .rounded(design::RADIUS_LG)
        .bg(p.surface)
        .border_1()
        .border_color(p.border)
}

/// 设置项：左边标题和说明，右边控件
fn row(
    title: &'static str,
    desc: impl Into<SharedString>,
    control: impl IntoElement,
    last: bool,
    p: &Palette,
) -> Div {
    h_flex()
        .px_4()
        .py_3()
        .gap_4()
        .when(!last, |d| d.border_b_1().border_color(p.divider))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().text_color(p.text).child(title))
                .child(div().text_xs().text_color(p.text_faint).child(desc.into())),
        )
        .child(div().flex_none().child(control))
}

/// 分段选择器
fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, String)],
    current: T,
    p: &Palette,
    cx: &mut Context<SettingsView>,
    on_pick: fn(&mut Config, T),
) -> Div {
    let p = *p;
    h_flex()
        .p_0p5()
        .gap_0p5()
        .rounded(design::RADIUS_MD)
        .bg(p.hover)
        .children(options.iter().enumerate().map(|(i, (value, label))| {
            let value = *value;
            let active = value == current;
            div()
                .id((id, i))
                .px_3()
                .py_1()
                .rounded(design::RADIUS_SM)
                .text_xs()
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(p.panel)
                        .text_color(p.text)
                        .font_weight(FontWeight::MEDIUM)
                        .shadow_sm()
                })
                .when(!active, |d| d.text_color(p.text_muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !active {
                        this.change(cx, |c| on_pick(c, value));
                    }
                }))
                .child(label.clone())
        }))
}

fn kbd(label: String, p: &Palette) -> Div {
    div()
        .h(px(24.))
        .min_w(px(24.))
        .px_2()
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

impl SettingsView {
    fn render_hotkey(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        h_flex()
            .id("hotkey-recorder")
            .track_focus(&self.recorder_focus)
            .min_w(px(180.))
            .h(px(36.))
            .px_2()
            .gap_1()
            .justify_center()
            .rounded(design::RADIUS_MD)
            .border_1()
            .cursor_pointer()
            .map(|d| {
                if self.recording {
                    d.border_color(p.accent)
                        .bg(p.accent_soft)
                        .text_xs()
                        .text_color(p.accent_text)
                        .child("请按下新的组合键（Esc 取消）")
                } else {
                    d.border_color(p.border)
                        .bg(p.search_bg)
                        .hover(move |s| s.border_color(p.accent))
                        .children(
                            self.config
                                .hotkey
                                .split('+')
                                .map(|k| kbd(k.trim().to_string(), &p)),
                        )
                }
            })
            .on_click(cx.listener(|this, _, window, cx| this.start_recording(window, cx)))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_record_key(event, cx)),
            )
    }

    fn render_ignored_apps(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_sm().text_color(p.text).child("不记录这些程序"))
                    .child(div().text_xs().text_color(p.text_faint).child(
                        "从这些程序复制的内容会被直接跳过，连内容都不会读取。按 exe 文件名匹配。",
                    )),
            )
            .child(h_flex().flex_wrap().gap_1p5().children(
                self.config.ignored_apps.iter().enumerate().map(|(i, app)| {
                    h_flex()
                        .id(("app", i))
                        .h(px(26.))
                        .pl_2p5()
                        .pr_1()
                        .gap_1()
                        .rounded_full()
                        .bg(p.hover)
                        .border_1()
                        .border_color(p.border)
                        .text_xs()
                        .text_color(p.text)
                        .child(app.clone())
                        .child(
                            div()
                                .id(("remove-app", i))
                                .size(px(18.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(p.text_faint)
                                .hover(move |s| s.bg(p.selected).text_color(p.text))
                                .on_click({
                                    let app = app.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.remove_ignored_app(app.clone(), cx)
                                    })
                                })
                                .child(Icon::new(IconName::Close).xsmall()),
                        )
                }),
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.add_app).small()))
                    .child(Button::new("add-app").label("添加").small().on_click(
                        cx.listener(|this, _, window, cx| this.add_ignored_app(window, cx)),
                    )),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(cx.theme().is_dark());
        let theme_options: Vec<(ThemePref, String)> = ThemePref::ALL
            .iter()
            .map(|t| (*t, t.label().to_string()))
            .collect();
        let mut keep_options: Vec<(usize, String)> =
            KEEP_OPTIONS.iter().map(|n| (*n, format!("{n}"))).collect();
        if !KEEP_OPTIONS.contains(&self.config.max_items) {
            keep_options.push((self.config.max_items, format!("{}", self.config.max_items)));
        }
        let auto_paste = self.config.auto_paste;
        let autostart = self.autostart;

        let general = card("通用", &p).child(
            card_body(&p)
                .child(row(
                    "唤起热键",
                    "在任何程序里按下它，打开剪贴板历史。点击右侧后按下新的组合键即可修改。",
                    self.render_hotkey(&p, cx),
                    false,
                    &p,
                ))
                .child(row(
                    "选中后直接粘贴",
                    "关闭后，选中的内容只复制到剪贴板，由你自己粘贴。",
                    Switch::new("auto-paste")
                        .checked(auto_paste)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            let v = *checked;
                            this.change(cx, |c| c.auto_paste = v)
                        })),
                    false,
                    &p,
                ))
                .child(row(
                    "开机自动启动",
                    "登录 Windows 后在后台启动，只显示托盘图标。",
                    Switch::new("autostart")
                        .checked(autostart)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            let _ = this.tx.try_send(AppCommand::SetAutostart(*checked));
                            this.autostart = *checked;
                            cx.notify();
                        })),
                    true,
                    &p,
                )),
        );

        let appearance = card("外观", &p).child(card_body(&p).child(row(
            "主题",
            "跟随系统时，Windows 切换深浅色后会自动跟着变。",
            segmented(
                "theme",
                &theme_options,
                self.config.theme,
                &p,
                cx,
                |c, v| c.theme = v,
            ),
            true,
            &p,
        )));

        let privacy = card("历史与隐私", &p).child(
            card_body(&p)
                .child(row(
                    "最多保留",
                    "超出后自动删除最旧的记录。收藏的内容不计入，也永远不会被自动删除。",
                    segmented(
                        "keep",
                        &keep_options,
                        self.config.max_items,
                        &p,
                        cx,
                        |c, v| c.max_items = v,
                    ),
                    false,
                    &p,
                ))
                .child(self.render_ignored_apps(&p, cx))
                .child(
                    div()
                        .px_4()
                        .pb_3()
                        .text_xs()
                        .text_color(p.text_faint)
                        .child(
                            "此外，密码管理器等程序标记为「不要记录」的内容，Tessel 一律不会读取。",
                        ),
                ),
        );

        let dir = self.dir.clone();
        let data = card("数据", &p).child(
            card_body(&p)
                .child(row(
                    "数据位置",
                    format!("{} · {}", self.dir.display(), self.storage),
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("open-dir")
                                .label("打开文件夹")
                                .small()
                                .outline()
                                .on_click(move |_, _, cx| cx.open_with_system(&dir)),
                        )
                        .child(
                            Button::new("open-config")
                                .label("编辑 config.toml")
                                .small()
                                .outline()
                                .on_click(cx.listener(|this, _, _, _| {
                                    let _ = this.tx.try_send(AppCommand::OpenConfigFile);
                                })),
                        ),
                    false,
                    &p,
                ))
                .child(row(
                    "清空历史",
                    "删除所有未收藏的记录，收藏的内容会保留。",
                    Button::new("clear")
                        .label("清空…")
                        .small()
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| {
                            let _ = this.tx.try_send(AppCommand::ClearHistory);
                            cx.notify();
                        })),
                    true,
                    &p,
                )),
        );

        let about = card("关于", &p).child(
            card_body(&p).child(
                h_flex()
                    .px_4()
                    .py_4()
                    .gap_3()
                    .child(img(self.logo.clone()).size(px(40.)).flex_none())
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child(format!("Tessel 拼贴 {}", env!("CARGO_PKG_VERSION"))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child("每一次复制，都拼进你的工作里。· MIT OR Apache-2.0"),
                            ),
                    ),
            ),
        );

        v_flex()
            .size_full()
            .bg(p.panel)
            .text_color(p.text)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_2()
                        .child(img(self.logo.clone()).size(px(16.)))
                        .child(div().text_sm().child("Tessel 设置")),
                ),
            )
            .when_some(self.error.clone().or(self.notice.clone()), |d, err| {
                d.child(
                    h_flex()
                        .mx_6()
                        .mt_4()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .rounded(design::RADIUS_MD)
                        .bg(rgba(0xDC26261A))
                        .text_xs()
                        .text_color(rgb(0xDC2626))
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(div().flex_1().child(err)),
                )
            })
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .px_6()
                            .py_5()
                            .gap_6()
                            .child(general)
                            .child(appearance)
                            .child(privacy)
                            .child(data)
                            .child(about),
                    ),
            )
    }
}

/// 打开设置窗口，返回窗口句柄和视图
pub fn open(
    dir: PathBuf,
    tx: async_channel::Sender<AppCommand>,
    config: Config,
    autostart: bool,
    cx: &mut App,
) -> Option<(AnyWindowHandle, Entity<SettingsView>)> {
    let bounds = Bounds::centered(None, size(px(720.), px(700.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitleBar::title_bar_options()),
        window_min_size: Some(size(px(560.), px(480.))),
        kind: WindowKind::Normal,
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title("Tessel 设置");
        cx.new(|cx| SettingsView::new(dir, tx, config, autostart, window, cx))
    })
    .inspect_err(|err| log_line(format!("[error] 打开设置窗口失败：{err}")))
    .ok()
}
