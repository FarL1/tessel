//! M0 技术验证 #2 / #5 / #6：产品核心闭环的雏形。
//!
//! 启动后窗口隐藏，按 Alt+V 弹出：输入过滤 → ↑↓ 选择 → Enter 粘贴回之前的窗口；Esc 或点击外部隐藏。
//!
//! 测量与验证：
//! - #2 热键 → 首帧渲染的延迟；隐藏状态下的常驻内存
//! - #5 隐藏自身 → 切回之前的前台窗口 → SendInput Ctrl+V 的成功率与耗时
//! - #6 全局热键（global-hotkey）、PopUp 窗口（不进任务栏、置顶）、Mica 背景
//!
//! 所有测量结果同时打印到 stdout，便于脚本化测试。

#[cfg(windows)]
mod win;

use std::time::Instant;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const ITEMS: &[&str] = &[
    "MyPaste 粘贴测试：你好，世界！",
    "https://github.com/longbridge/gpui-kit",
    "验证码 482913，5 分钟内有效",
    "收货地址：上海市浦东新区世纪大道 100 号",
    "SELECT * FROM clip_items ORDER BY seq DESC LIMIT 50;",
    "#FF6B6B",
    "The quick brown fox jumps over the lazy dog",
    "😀 Emoji 也要能正确粘贴 🎉",
];

const MAX_LOG: usize = 6;

fn log_line(line: String) {
    println!("{line}");
}

struct Popup {
    hwnd: isize,
    visible: bool,
    /// 弹出前的前台窗口，粘贴时切回它
    prev_foreground: Option<isize>,
    search: Entity<InputState>,
    query: SharedString,
    selected: usize,
    log: Vec<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl Popup {
    fn new(hwnd: isize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索…（↑↓ 选择，Enter 粘贴，Esc 关闭）")
        });

        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, state, event, window, cx| {
                match event {
                    InputEvent::Change => {
                        this.query = state.read(cx).value();
                        this.selected = 0;
                    }
                    InputEvent::PressEnter { .. } => {
                        log_line("[event] PressEnter".into());
                        this.paste(window, cx)
                    }
                    _ => {}
                }
                cx.notify();
            }),
            // 点击外部（窗口失去激活）时自动隐藏
            cx.observe_window_activation(window, |this, window, cx| {
                log_line(format!(
                    "[event] 窗口激活状态变化：active={} visible={}",
                    window.is_window_active(),
                    this.visible
                ));
                if this.visible && !window.is_window_active() {
                    this.hide(false, cx);
                }
            }),
        ];

        Self {
            hwnd,
            visible: false,
            prev_foreground: None,
            search,
            query: SharedString::default(),
            selected: 0,
            log: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    fn push_log(&mut self, line: String) {
        log_line(line.clone());
        self.log.push(line.into());
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    fn matches(&self) -> Vec<&'static str> {
        let q = self.query.trim().to_lowercase();
        ITEMS
            .iter()
            .copied()
            .filter(|s| q.is_empty() || s.to_lowercase().contains(&q))
            .collect()
    }

    fn toggle(&mut self, t0: Instant, window: &mut Window, cx: &mut Context<Self>) {
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

        // 注意：不要调用 window.activate_window()。GPUI 在 Windows 上的实现是异步的，
        // 首次调用会重新应用窗口创建时保存的位置状态，还会注入一次 Alt 键来绕过前台锁，
        // 这个假 Alt 会发给当时的前台窗口。收到 WM_HOTKEY 时我们本身就有前台权限，直接用 Win32 即可。
        win::show_and_focus(self.hwnd);
        self.search.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });

        let shown = t0.elapsed();
        let prev_title = self
            .prev_foreground
            .map(win::window_title)
            .unwrap_or_default();
        let view = cx.entity().downgrade();
        window.on_next_frame(move |_window, cx| {
            let frame = t0.elapsed();
            let _ = view.update(cx, |this, cx| {
                this.push_log(format!(
                    "[show] 热键→ShowWindow {:.1}ms · 热键→首帧 {:.1}ms · 来源窗口「{prev_title}」",
                    shown.as_secs_f64() * 1000.0,
                    frame.as_secs_f64() * 1000.0,
                ));
                cx.notify();
            });
        });
        cx.notify();
    }

    /// `restore_focus`：Esc / 热键关闭时把焦点还给之前的窗口；点击外部时不需要（用户已经选了新窗口）。
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
        self.push_log(format!("[hide] 工作集 {ws:.1}MB · 私有 {private:.1}MB"));
        cx.notify();
    }

    fn paste(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.matches();
        let Some(&text) = matches.get(self.selected) else {
            return;
        };
        let Some(target) = self.prev_foreground.take() else {
            self.push_log("[paste] 没有记录到来源窗口，只复制到剪贴板".into());
            let _ = win::set_clipboard_text(self.hwnd, text);
            return;
        };

        let t0 = Instant::now();
        if let Err(err) = win::set_clipboard_text(self.hwnd, text) {
            self.push_log(format!("[paste] ❌ 写剪贴板失败：{err}"));
            return;
        }
        self.visible = false;
        win::hide(self.hwnd);
        let report = win::restore_and_paste(target);
        self.push_log(format!(
            "[paste] {} 目标「{}」· 切换焦点 {:.1}ms · 总耗时 {:.1}ms",
            if report.focused {
                "✅"
            } else {
                "❌ 无法切回目标窗口"
            },
            report.target_title,
            report.focus_ms,
            t0.elapsed().as_secs_f64() * 1000.0,
        ));
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.matches().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        cx.notify();
    }
}

impl Render for Popup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches();
        let theme = cx.theme();
        let (muted, border, accent) = (theme.muted_foreground, theme.border, theme.accent);
        let selected = self.selected.min(matches.len().saturating_sub(1));

        div()
            .size_full()
            .v_flex()
            .gap_2()
            .p_3()
            // 在输入框之前截获导航键
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                log_line(format!("[event] key_down {:?}", event.keystroke.key));
                match event.keystroke.key.as_str() {
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    "escape" => this.hide(true, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(Input::new(&self.search))
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .children(matches.into_iter().enumerate().map(|(i, text)| {
                        div()
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .when(i == selected, |d| d.bg(accent))
                            .child(text)
                    })),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .pt_2()
                    .text_xs()
                    .text_color(muted)
                    .v_flex()
                    .children(self.log.iter().cloned()),
            )
    }
}

fn main() {
    let (tx, rx) = async_channel::unbounded::<Instant>();

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);

        // 全局热键：global-hotkey 在当前线程创建隐藏窗口接收 WM_HOTKEY，由 GPUI 的消息循环派发
        let manager = GlobalHotKeyManager::new().expect("hotkey manager");
        let hotkey = HotKey::new(Some(Modifiers::ALT), Code::KeyV);
        manager
            .register(hotkey)
            .expect("register Alt+V（可能已被其他程序占用）");
        let hotkey_id = hotkey.id();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.id == hotkey_id && event.state == HotKeyState::Pressed {
                let _ = tx.try_send(Instant::now());
            }
        }));
        // manager 析构会注销热键，让它活到进程结束
        std::mem::forget(manager);

        let bounds = Bounds::centered(None, size(px(600.), px(460.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            // 以 show: true 创建，在构建闭包里立刻用 Win32 隐藏（首帧之前，不会闪烁）。
            // 如果用 show: false，GPUI 会保存"延迟应用的位置状态"，并在第一次 activate 时异步应用，
            // 打乱我们自己的显示和焦点流程。
            show: true,
            focus: false,
            is_resizable: false,
            window_background: WindowBackgroundAppearance::MicaBackdrop,
            ..Default::default()
        };

        let (window_handle, popup) = gpui_kit::open_window(options, cx, |window, cx| {
            let hwnd = match window.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                _ => panic!("expected a Win32 window handle"),
            };
            win::hide(hwnd);
            cx.new(|cx| Popup::new(hwnd, window, cx))
        })
        .expect("failed to open window");

        let (ws, private) = win::memory_mb();
        log_line(format!(
            "[ready] 按 Alt+V 唤起 · 启动后工作集 {ws:.1}MB · 私有 {private:.1}MB"
        ));

        cx.spawn(async move |cx| {
            while let Ok(t0) = rx.recv().await {
                let popup = popup.clone();
                let _ = window_handle.update(cx, |_, window, cx| {
                    popup.update(cx, |this, cx| this.toggle(t0, window, cx));
                });
            }
        })
        .detach();
    });
}
