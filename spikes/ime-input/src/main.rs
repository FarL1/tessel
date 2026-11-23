//! M0 技术验证 #1：在 Windows 上测试 GPUI（gpui-kit）输入框对中文输入法的支持。
//!
//! 要验证的点（结果记录在 docs/m0-report.md）：
//! - 微软拼音 / 搜狗 / 小鹤等输入法能否正常上屏，候选框是否跟随光标
//! - 输入法组字（preedit）期间不应触发搜索；上屏后才过滤
//! - 中英文混排、全角/半角标点、Emoji 输入
//! - 光标移动、选中、删除、Ctrl+A/C/V/Z
//! - 高 DPI（125% / 150% / 200%）缩放与多显示器之间拖动窗口时的显示

use gpui_kit::base::StyledExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

const SAMPLES: &[&str] = &[
    "剪贴板历史记录支持全文搜索",
    "https://github.com/longbridge/gpui-kit",
    "今天下午三点开会，记得带上周报",
    "fn main() { println!(\"你好，世界\"); }",
    "#FF6B6B",
    "收货地址：上海市浦东新区世纪大道 100 号",
    "SELECT * FROM clip_items WHERE kind = 'text' ORDER BY created_at DESC;",
    "Rust 是一门注重安全、速度和并发的系统编程语言",
    "The quick brown fox jumps over the lazy dog",
    "验证码 482913，5 分钟内有效",
    "あいうえお 日本語のテキスト",
    "😀 表情符号 Emoji 测试 🎉",
    "全角标点：，。！？；：“”‘’（）【】",
    "半角标点: ,.!?;:\"'()[]",
    "多设备同步：局域网直连，端到端加密",
    "粘贴队列：按顺序依次粘贴多条内容",
];

struct ImeSpike {
    search: Entity<InputState>,
    notes: Entity<TextareaState>,
    query: SharedString,
    last_event: SharedString,
    change_count: usize,
    _subscriptions: Vec<Subscription>,
}

impl ImeSpike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("用中文输入法搜索，例如：剪贴板、同步、验证码…")
        });
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("多行输入区：随便输入一段中英文混排的长文本，测试换行、选中和撤销")
        });

        let subscriptions =
            vec![
                cx.subscribe_in(&search, window, |this, state, event, _window, cx| {
                    match event {
                        InputEvent::Change => {
                            this.query = state.read(cx).value();
                            this.change_count += 1;
                            this.last_event = "Change".into();
                        }
                        InputEvent::PressEnter { .. } => this.last_event = "PressEnter".into(),
                        InputEvent::Focus => this.last_event = "Focus".into(),
                        InputEvent::Blur => this.last_event = "Blur".into(),
                    }
                    cx.notify();
                }),
            ];

        // 打开窗口后焦点直接落在搜索框，模拟"唤起即可输入"
        search.update(cx, |state, cx| state.focus(window, cx));

        Self {
            search,
            notes,
            query: SharedString::default(),
            last_event: "—".into(),
            change_count: 0,
            _subscriptions: subscriptions,
        }
    }

    fn matches(&self) -> Vec<&'static str> {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return SAMPLES.to_vec();
        }
        SAMPLES
            .iter()
            .copied()
            .filter(|s| s.to_lowercase().contains(&q))
            .collect()
    }
}

impl Render for ImeSpike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.border;

        let status = format!(
            "查询：「{}」 · {} 字符 / {} 字节 · Change 事件 {} 次 · 最近事件：{} · 匹配 {} 条",
            self.query,
            self.query.chars().count(),
            self.query.len(),
            self.change_count,
            self.last_event,
            matches.len(),
        );

        div()
            .size_full()
            .v_flex()
            .gap_3()
            .p_4()
            .child(Input::new(&self.search).cleanable(true))
            .child(div().text_xs().text_color(muted).child(status))
            .child(
                div()
                    .id("results")
                    .flex_1()
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .children(matches.into_iter().enumerate().map(|(i, text)| {
                        div()
                            .px_3()
                            .py_1p5()
                            .when(i > 0, |d| d.border_t_1().border_color(border))
                            .child(text)
                    })),
            )
            .child(Textarea::new(&self.notes).h(px(120.)))
    }
}

/// 启动阶段计时（设置环境变量 MYPASTE_TRACE=1 时打印）
fn trace(start: std::time::Instant, stage: &str) {
    if std::env::var_os("MYPASTE_TRACE").is_some() {
        eprintln!("[trace] {:>7.1}ms  {stage}", start.elapsed().as_secs_f64() * 1000.0);
    }
}

fn main() {
    let start = std::time::Instant::now();
    trace(start, "main()");
    let app = gpui_kit::application();
    trace(start, "application() 构建完成");
    app.run(move |cx| {
        trace(start, "进入 run 回调");
        gpui_kit::init(cx);
        trace(start, "gpui_kit::init 完成");

        let bounds = Bounds::centered(None, size(px(720.), px(560.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("MyPaste · M0 · 中文输入法验证".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let (window_handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
            trace(start, "原生窗口已创建，开始构建视图");
            cx.new(|cx| ImeSpike::new(window, cx))
        })
        .expect("failed to open window");
        trace(start, "open_window 返回");

        let _ = window_handle.update(cx, |_, window, _| {
            window.on_next_frame(move |_, _| trace(start, "首帧渲染完成"));
        });
        cx.activate(true);
    });
}
