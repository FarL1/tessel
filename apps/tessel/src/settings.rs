//! 设置窗口。
//!
//! 界面读写的就是 `config.toml`：每次修改立即写回文件（保留注释），再走和"手动编辑文件"
//! 完全相同的热加载流程。所以界面修改和文本编辑永远一致，也只有一处校验逻辑。

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, TitleBar};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_core::backup::ExportScope;
use tessel_core::i18n::tr;
use tessel_core::trf;
use tessel_core::{Config, LanguagePref, ThemePref};

use crate::design::{self, Palette};
use crate::{AppCommand, log_line};

/// 历史条数的可选值
const KEEP_OPTIONS: [usize; 4] = [1_000, 5_000, 20_000, 100_000];

/// 正在录制哪个热键
#[derive(Clone, Copy, PartialEq, Eq)]
enum HotkeyTarget {
    /// 唤起弹窗
    Popup,
    /// 粘贴队列的下一条
    Queue,
}

impl HotkeyTarget {
    fn ix(self) -> usize {
        self as usize
    }

    fn spec(self, config: &Config) -> &str {
        match self {
            HotkeyTarget::Popup => &config.hotkey,
            HotkeyTarget::Queue => &config.queue_hotkey,
        }
    }

    fn set(self, config: &mut Config, spec: String) {
        match self {
            HotkeyTarget::Popup => config.hotkey = spec,
            HotkeyTarget::Queue => config.queue_hotkey = spec,
        }
    }
}

pub struct SettingsView {
    dir: PathBuf,
    tx: async_channel::Sender<AppCommand>,
    config: Config,
    autostart: bool,
    /// 正在录制哪个热键
    recording: Option<HotkeyTarget>,
    recorder_focus: [FocusHandle; 2],
    add_app: Entity<InputState>,
    /// 上次渲染时的界面语言：变了就更新窗口标题和输入框占位文字
    rendered_english: Option<bool>,
    /// 配置未生效的原因（文件有错、热键被占用等），随下一次重载结果更新
    error: Option<SharedString>,
    /// 一次性提示（例如"新热键被占用，已恢复为旧热键"），不会被配置重载清掉，下次录制时清除
    notice: Option<SharedString>,
    /// 操作成功的提示（导入、导出完成），蓝色显示
    info: Option<SharedString>,
    /// 正在导入或导出：按钮暂时不可用
    busy: bool,
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
        let add_app = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr(
                "程序文件名，例如 KeePassXC.exe",
                "Program file name, e.g. KeePassXC.exe",
            ))
        });
        let recorder_focus = [cx.focus_handle(), cx.focus_handle()];
        let subscriptions = vec![
            cx.subscribe_in(&add_app, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_ignored_app(window, cx);
                }
            }),
            // 录制时点到别处、切到别的窗口：取消录制
            cx.on_blur(&recorder_focus[0], window, |this, _, cx| {
                if this.recording == Some(HotkeyTarget::Popup) {
                    this.stop_recording(cx)
                }
            }),
            cx.on_blur(&recorder_focus[1], window, |this, _, cx| {
                if this.recording == Some(HotkeyTarget::Queue) {
                    this.stop_recording(cx)
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.stop_recording(cx);
                }
            }),
            // 录制中直接关掉窗口：视图销毁时也要恢复全局热键
            cx.on_release(|this, _| {
                if this.recording.is_some() {
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
            recording: None,
            recorder_focus,
            add_app,
            rendered_english: None,
            error: None,
            notice: None,
            info: None,
            busy: false,
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
                self.error = Some(
                    trf!(
                        "{err}。请先修正 config.toml，再在这里修改。",
                        "{err}. Fix config.toml first, then change it here."
                    )
                    .into(),
                );
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

    // ── 导入导出 ──

    /// 导出：选保存位置，在后台写文件
    fn export(&mut self, scope: ExportScope, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let t = crate::win::local_time();
        let name = match scope {
            ExportScope::All => format!(
                "{}-{:04}{:02}{:02}.jsonl",
                tr("Tessel 备份", "Tessel backup"),
                t.year,
                t.month,
                t.day
            ),
            ExportScope::Kept => format!(
                "{}-{:04}{:02}{:02}.jsonl",
                tr("Tessel 收藏和片段", "Tessel pinned and snippets"),
                t.year,
                t.month,
                t.day
            ),
        };
        let start_dir = documents_dir();
        let picked = cx.prompt_for_new_path(&start_dir, Some(&name));
        let dir = self.dir.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = true;
                this.info = Some(tr("正在导出…", "Exporting…").into());
                cx.notify();
            });
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = tessel_core::Store::open(&dir).map_err(|e| e.to_string())?;
                    let file = std::fs::File::create(&path)
                        .map_err(|e| trf!("无法创建文件：{e}", "Could not create the file: {e}"))?;
                    let mut out = std::io::BufWriter::new(file);
                    let n = store.export(scope, env!("CARGO_PKG_VERSION"), &mut out)?;
                    Ok::<_, String>((n, path))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((n, path)) => {
                        let path = path.display();
                        this.info = Some(
                            trf!("已导出 {n} 条到 {path}", "Exported {n} items to {path}").into(),
                        );
                    }
                    Err(err) => {
                        this.info = None;
                        this.notice = Some(err.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 导入：选文件，在后台合并进历史
    fn import(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr("导入", "Import").into()),
        });
        let dir = self.dir.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = true;
                this.info = Some(tr("正在导入…", "Importing…").into());
                cx.notify();
            });
            let result = cx
                .background_executor()
                .spawn(async move {
                    let file = std::fs::File::open(&path)
                        .map_err(|e| trf!("无法打开文件：{e}", "Could not open the file: {e}"))?;
                    let mut store = tessel_core::Store::open(&dir).map_err(|e| e.to_string())?;
                    store.import(std::io::BufReader::new(file))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(r) => {
                        let (added, merged, skipped) = (r.added, r.merged, r.skipped);
                        let mut text = trf!(
                            "导入完成：新增 {added} 条，{merged} 条已存在（已合并收藏、片段和备注）",
                            "Imported: {added} new, {merged} already present (pins, snippets and notes merged)"
                        );
                        if skipped > 0 {
                            text.push_str(&trf!("，{skipped} 行无法识别已跳过", "; {skipped} unreadable lines skipped"));
                        }
                        this.info = Some(text.into());
                        this.refresh_storage(cx);
                        // 弹窗下次显示时会重新查询；正在显示的话立刻刷新
                        let _ = this.tx.try_send(AppCommand::ClipboardChanged { notify: false });
                    }
                    Err(err) => {
                        this.info = None;
                        this.notice = Some(err.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_recording(
        &mut self,
        target: HotkeyTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.recording == Some(target) {
            return;
        }
        // 从一个录制框直接点到另一个：先结束上一个
        self.stop_recording(cx);
        self.recording = Some(target);
        self.notice = None;
        self.recorder_focus[target.ix()].focus(window, cx);
        // 暂时注销全局热键，否则按下当前热键会被系统截走，录不到
        let _ = self.tx.try_send(AppCommand::HotkeyRecording(true));
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.is_none() {
            return;
        }
        self.recording = None;
        let _ = self.tx.try_send(AppCommand::HotkeyRecording(false));
        cx.notify();
    }

    fn on_record_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(target) = self.recording else {
            return;
        };
        cx.stop_propagation();
        let ks = &event.keystroke;
        if ks.key == "escape" {
            self.stop_recording(cx);
            return;
        }
        // 粘贴队列的热键可以清除（不使用）
        if target == HotkeyTarget::Queue
            && matches!(ks.key.as_str(), "backspace" | "delete")
            && !ks.modifiers.modified()
        {
            self.change(cx, |c| c.queue_hotkey = String::new());
            self.stop_recording(cx);
            return;
        }
        let Some(key) = hotkey_token(&ks.key) else {
            self.error = Some(
                trf!(
                    "不支持「{}」这个键，请换一个",
                    "The key \"{}\" isn't supported; please pick another",
                    ks.key
                )
                .into(),
            );
            cx.notify();
            return;
        };
        let m = &ks.modifiers;
        let is_fn_key = key.starts_with('F') && key.len() > 1;
        if !(m.control || m.alt || m.shift || m.platform) && !is_fn_key {
            self.error = Some(
                tr(
                    "热键需要包含 Ctrl / Alt / Shift / Win 中的至少一个",
                    "A hotkey needs at least one of Ctrl / Alt / Shift / Win",
                )
                .into(),
            );
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
        let other = match target {
            HotkeyTarget::Popup => HotkeyTarget::Queue,
            HotkeyTarget::Queue => HotkeyTarget::Popup,
        };
        if other.spec(&self.config).eq_ignore_ascii_case(&spec) {
            self.error = Some(
                trf!(
                    "「{spec}」已经用作另一个热键了，请换一个",
                    "\"{spec}\" is already used for the other hotkey; please pick another"
                )
                .into(),
            );
            cx.notify();
            return;
        }
        // 先发 ConfigChanged 再发恢复热键：主线程按顺序处理，会直接注册新热键
        self.change(cx, |c| target.set(c, spec));
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

/// 导出时默认打开的文件夹：文档
fn documents_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|home| PathBuf::from(home).join("Documents"))
        .filter(|p| p.exists())
        .unwrap_or_else(std::env::temp_dir)
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
    trf!("{count} 条记录 · 占用 {size}", "{count} items · {size}")
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
    fn render_hotkey(
        &self,
        target: HotkeyTarget,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *p;
        let spec = target.spec(&self.config).to_string();
        h_flex()
            .id(("hotkey-recorder", target.ix()))
            .track_focus(&self.recorder_focus[target.ix()])
            .min_w(px(180.))
            .h(px(36.))
            .px_2()
            .gap_1()
            .justify_center()
            .rounded(design::RADIUS_MD)
            .border_1()
            .cursor_pointer()
            .map(|d| {
                if self.recording == Some(target) {
                    d.border_color(p.accent)
                        .bg(p.accent_soft)
                        .text_xs()
                        .text_color(p.accent_text)
                        .child(match target {
                            HotkeyTarget::Popup => tr(
                                "请按下新的组合键（Esc 取消）",
                                "Press the new key combination (Esc to cancel)",
                            ),
                            HotkeyTarget::Queue => tr(
                                "按下组合键（Esc 取消，Backspace 清除）",
                                "Press a key combination (Esc cancels, Backspace clears)",
                            ),
                        })
                } else if spec.is_empty() {
                    d.border_color(p.border)
                        .bg(p.search_bg)
                        .hover(move |s| s.border_color(p.accent))
                        .text_xs()
                        .text_color(p.text_faint)
                        .child(tr("未设置，点击设置", "Not set. Click to set"))
                } else {
                    d.border_color(p.border)
                        .bg(p.search_bg)
                        .hover(move |s| s.border_color(p.accent))
                        .children(spec.split('+').map(|k| kbd(k.trim().to_string(), &p)))
                }
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.start_recording(target, window, cx)),
            )
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
                    .child(div().text_sm().text_color(p.text).child(tr("不记录这些程序", "Never record these programs")))
                    .child(div().text_xs().text_color(p.text_faint).child(
                        tr("从这些程序复制的内容会被直接跳过，连内容都不会读取。按 exe 文件名匹配。", "Copies from these programs are skipped without reading their content. Matched by exe file name."),
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
                    .child(Button::new("add-app").label(tr("添加", "Add")).small().on_click(
                        cx.listener(|this, _, window, cx| this.add_ignored_app(window, cx)),
                    )),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(cx.theme().is_dark());
        let english = tessel_core::i18n::is_en();
        if self.rendered_english != Some(english) {
            self.rendered_english = Some(english);
            window.set_window_title(tr("Tessel 设置", "Tessel Settings"));
            self.add_app.update(cx, |state, cx| {
                state.set_placeholder(
                    tr(
                        "程序文件名，例如 KeePassXC.exe",
                        "Program file name, e.g. KeePassXC.exe",
                    ),
                    window,
                    cx,
                )
            });
        }
        let language_options: Vec<(LanguagePref, String)> = LanguagePref::ALL
            .iter()
            .map(|l| (*l, l.label().to_string()))
            .collect();
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
        let copy_toast = self.config.copy_toast;
        let autostart = self.autostart;

        let general = card(tr("通用", "General"), &p).child(
            card_body(&p)
                .child(row(
                    tr("唤起热键", "Hotkey"),
                    tr("在任何程序里按下它，打开剪贴板历史。点击右侧后按下新的组合键即可修改。", "Press it in any program to open your clipboard history. Click the box and press a new combination to change it."),
                    self.render_hotkey(HotkeyTarget::Popup, &p, cx),
                    false,
                    &p,
                ))
                .child(row(
                    tr("粘贴队列热键", "Paste-queue hotkey"),
                    tr("在弹窗里按 Ctrl+Q 把记录排进队列，之后在任何程序里每按一次这个热键，就按顺序粘贴下一条。", "Press Ctrl+Q in the popup to queue items; then each press of this hotkey, in any program, pastes the next one."),
                    self.render_hotkey(HotkeyTarget::Queue, &p, cx),
                    false,
                    &p,
                ))
                .child(row(
                    tr("选中后直接粘贴", "Paste on selection"),
                    tr("关闭后，选中的内容只复制到剪贴板，由你自己粘贴。", "When off, the selected item is only copied to the clipboard for you to paste."),
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
                    tr("复制时显示提示", "Show a hint when copying"),
                    tr("每次复制成功，在屏幕底部显示一下复制了什么。", "After each copy, briefly show what was copied at the bottom of the screen."),
                    Switch::new("copy-toast")
                        .checked(copy_toast)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            let v = *checked;
                            this.change(cx, |c| c.copy_toast = v)
                        })),
                    false,
                    &p,
                ))
                .child(row(
                    tr("开机自动启动", "Start with Windows"),
                    tr("登录 Windows 后在后台启动，只显示托盘图标。", "Start in the background after you sign in; only the tray icon shows."),
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

        let appearance = card(tr("外观", "Appearance"), &p).child(
            card_body(&p)
                .child(row(
                    tr("主题", "Theme"),
                    tr(
                        "跟随系统时，Windows 切换深浅色后会自动跟着变。",
                        "With System, Tessel follows Windows when it switches between light and dark.",
                    ),
                    segmented(
                        "theme",
                        &theme_options,
                        self.config.theme,
                        &p,
                        cx,
                        |c, v| c.theme = v,
                    ),
                    false,
                    &p,
                ))
                // 标题两种语言都写上：切错了语言也找得到
                .child(row(
                    "语言 / Language",
                    tr(
                        "跟随系统时，中文 Windows 显示中文，其他显示英文。",
                        "With System, Tessel uses Chinese on Chinese Windows and English otherwise.",
                    ),
                    segmented(
                        "language",
                        &language_options,
                        self.config.language,
                        &p,
                        cx,
                        |c, v| c.language = v,
                    ),
                    true,
                    &p,
                )),
        );

        let privacy = card(tr("历史与隐私", "History & privacy"), &p).child(
            card_body(&p)
                .child(row(
                    tr("最多保留", "Keep up to"),
                    tr("超出后自动删除最旧的记录。收藏的内容不计入，也永远不会被自动删除。", "The oldest items are removed beyond this. Pinned items don't count and are never removed automatically."),
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
                            tr("此外，密码管理器等程序标记为「不要记录」的内容，Tessel 一律不会读取。", "Content that password managers and similar programs mark as \"do not record\" is never read."),
                        ),
                ),
        );

        let dir = self.dir.clone();
        let data = card(tr("数据", "Data"), &p).child(
            card_body(&p)
                .child(row(
                    tr("数据位置", "Data folder"),
                    format!("{} · {}", self.dir.display(), self.storage),
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("open-dir")
                                .label(tr("打开文件夹", "Open folder"))
                                .small()
                                .outline()
                                .on_click(move |_, _, cx| cx.open_with_system(&dir)),
                        )
                        .child(
                            Button::new("open-config")
                                .label(tr("编辑 config.toml", "Edit config.toml"))
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
                    tr("导出", "Export"),
                    tr(
                        "保存成一个文件（含图片、格式、备注），用来备份或换电脑。也可以只导出收藏和片段，分享给别人。",
                        "Save everything (images, formatting and notes included) to one file for backup or a new PC, or only pinned items and snippets to share.",
                    ),
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("export-all")
                                .label(tr("导出全部…", "Export all…"))
                                .small()
                                .outline()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export(ExportScope::All, cx)
                                })),
                        )
                        .child(
                            Button::new("export-kept")
                                .label(tr("只导出收藏和片段…", "Pinned & snippets…"))
                                .small()
                                .outline()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export(ExportScope::Kept, cx)
                                })),
                        ),
                    false,
                    &p,
                ))
                .child(row(
                    tr("导入", "Import"),
                    tr(
                        "导入 Tessel 导出的文件。已有的内容不会重复，收藏、片段和备注会合并。",
                        "Import a file exported by Tessel. Existing items aren't duplicated; pins, snippets and notes are merged.",
                    ),
                    Button::new("import")
                        .label(tr("导入…", "Import…"))
                        .small()
                        .outline()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
                    false,
                    &p,
                ))
                .child(row(
                    tr("清空历史", "Clear history"),
                    tr(
                        "删除所有未收藏的记录，收藏的内容和片段会保留。",
                        "Delete every item that isn't pinned. Pinned items and snippets are kept.",
                    ),
                    Button::new("clear")
                        .label(tr("清空…", "Clear…"))
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

        let about = card(tr("关于", "About"), &p).child(
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
                                    .child(trf!(
                                        "Tessel 拼贴 {}",
                                        "Tessel {}",
                                        env!("CARGO_PKG_VERSION")
                                    )),
                            )
                            .child(div().text_xs().text_color(p.text_faint).child(tr(
                                "每一次复制，都拼进你的工作里。· MIT OR Apache-2.0",
                                "A clipboard manager for Windows · MIT OR Apache-2.0",
                            ))),
                    ),
            ),
        );

        v_flex()
            .size_full()
            .relative()
            .bg(p.panel)
            .text_color(p.text)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_2()
                        .child(img(self.logo.clone()).size(px(16.)))
                        .child(div().text_sm().child(tr("Tessel 设置", "Tessel Settings"))),
                ),
            )
            .when_some(
                self.info
                    .clone()
                    .filter(|_| self.error.is_none() && self.notice.is_none()),
                |d, info| {
                    d.child(
                        h_flex()
                            .mx_6()
                            .mt_4()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .rounded(design::RADIUS_MD)
                            .bg(p.accent_soft)
                            .text_xs()
                            .text_color(p.accent_text)
                            .child(Icon::new(IconName::Info).xsmall())
                            .child(div().flex_1().child(info)),
                    )
                },
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
            .with_animation(
                "settings-in",
                Animation::new(std::time::Duration::from_millis(220)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).top(px((1.0 - t) * 8.)),
            )
    }
}

/// 打开设置窗口，返回窗口句柄和视图
/// `over`：在这个窗口（正在显示的弹窗）的位置打开，看起来是弹窗变成了设置窗口；
/// 为空时放在鼠标所在屏幕的中间
pub fn open(
    dir: PathBuf,
    tx: async_channel::Sender<AppCommand>,
    config: Config,
    autostart: bool,
    over: Option<isize>,
    cx: &mut App,
) -> Option<(AnyWindowHandle, Entity<SettingsView>)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let bounds = Bounds::centered(None, size(px(720.), px(700.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitleBar::title_bar_options()),
        window_min_size: Some(size(px(560.), px(480.))),
        kind: WindowKind::Normal,
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        ..Default::default()
    };
    let mut hwnd = None;
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title(tr("Tessel 设置", "Tessel Settings"));
        // 先隐藏，摆好位置再显示（首帧之前，不会在屏幕中间闪一下）
        if let Ok(RawWindowHandle::Win32(h)) = window.window_handle().map(|h| h.as_raw()) {
            let raw = h.hwnd.get();
            crate::win::hide(raw);
            crate::win::place_over(raw, over);
            hwnd = Some(raw);
        }
        cx.new(|cx| SettingsView::new(dir, tx, config, autostart, window, cx))
    })
    .inspect_err(|err| log_line(format!("[error] 打开设置窗口失败：{err}")))
    .ok();
    // 显示并切到前台：系统的窗口打开动画照常播放；弹窗失去焦点后自己收起
    if let Some(raw) = hwnd {
        crate::win::restore_and_focus(raw);
    }
    opened
}
