//! 内存对照组：用 wry（WebView2，也就是 Tauri 的底层）渲染和 GPUI 原型差不多的界面，
//! 用相同的口径（整个进程树的私有工作集）比较内存。
//!
//! 运行：`cargo run -p spike-webview-baseline --release`

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

const ITEMS: &[(&str, &str, &str)] = &[
    (
        "验证码 482913，5 分钟内有效，请勿泄露给他人。",
        "微信",
        "刚刚",
    ),
    (
        "github.com/longbridge/gpui-kit",
        "Microsoft Edge",
        "2 分钟前",
    ),
    (
        "fn planned_search(conn: &Connection, query: &str)",
        "Visual Studio Code",
        "8 分钟前",
    ),
    ("#4F6BED", "Figma", "15 分钟前"),
    ("截图 2026-11-24 09.41.12", "截图工具", "32 分钟前"),
    (
        "收货地址：上海市浦东新区世纪大道 100 号",
        "钉钉",
        "1 小时前",
    ),
    ("首页-v3.fig 等 3 个文件", "文件资源管理器", "2 小时前"),
    ("会议纪要：周三下午 3 点评审 M1 交付范围。", "飞书", "昨天"),
    ("www.iroh.computer/blog/v1", "Microsoft Edge", "昨天"),
    ("SELECT id, content FROM clip_items", "DataGrip", "昨天"),
    ("#10B981", "Figma", "2 天前"),
    (
        "The quick brown fox jumps over the lazy dog.",
        "记事本",
        "3 天前",
    ),
];

fn html() -> String {
    let rows: String = ITEMS
        .iter()
        .enumerate()
        .map(|(i, (title, source, ago))| {
            format!(
                r#"<div class="row{sel}"><div class="badge"></div><div class="txt"><div class="t">{title}</div><div class="m">{source} · {ago}</div></div><kbd>{n}</kbd></div>"#,
                sel = if i == 0 { " sel" } else { "" },
                n = i + 1
            )
        })
        .collect();
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
*{{box-sizing:border-box;margin:0}} body{{font:14px "Segoe UI","Microsoft YaHei UI";background:#F8F8FA;color:#1B1C20;height:100vh;display:flex;flex-direction:column}}
header{{height:56px;display:flex;align-items:center;padding:0 16px;gap:12px}} input{{flex:1;border:0;background:none;font-size:16px;outline:none}}
.chips{{display:flex;gap:4px;padding:0 12px 8px;border-bottom:1px solid #0000000F}} .chip{{padding:4px 10px;border-radius:6px;font-size:12px;color:#5D5F68}} .chip.on{{background:#4F6BED1F;color:#3550D6}}
main{{flex:1;display:flex;min-height:0}} .list{{width:330px;overflow:auto;padding:8px 0;border-right:1px solid #0000000F}}
.row{{height:54px;margin:0 8px;padding:0 8px;display:flex;align-items:center;gap:12px;border-radius:8px}} .row.sel{{background:#4F6BED17}} .row:hover{{background:#0000000A}}
.badge{{width:32px;height:32px;border-radius:8px;background:#7B849424;flex:none}} .txt{{flex:1;min-width:0}} .t{{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}} .m{{font-size:12px;color:#9497A1}}
kbd{{font-size:11px;border:1px solid #0000001F;border-radius:4px;padding:0 5px;color:#5D5F68}} .pv{{flex:1;padding:16px;background:#FFFFFFB3;font-size:16px;line-height:1.65}}
footer{{height:38px;border-top:1px solid #0000000F;display:flex;align-items:center;gap:16px;padding:0 16px;font-size:12px;color:#5D5F68}}
</style></head><body>
<header><span>🔍</span><input placeholder="搜索剪贴板历史…" autofocus><span class="m">12 / 12 条</span></header>
<div class="chips"><span class="chip on">全部</span><span class="chip">文本 4</span><span class="chip">链接 2</span><span class="chip">代码 2</span><span class="chip">图片 1</span><span class="chip">文件 1</span><span class="chip">颜色 2</span><span class="chip">收藏 2</span></div>
<main><div class="list">{rows}</div><div class="pv">验证码 482913，5 分钟内有效，请勿泄露给他人。</div></main>
<footer><span><kbd>↵</kbd> 粘贴</span><span><kbd>Shift</kbd><kbd>↵</kbd> 纯文本</span><span><kbd>Ctrl</kbd><kbd>1–9</kbd> 快速粘贴</span></footer>
</body></html>"#
    )
}

fn main() -> wry::Result<()> {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("WebView baseline")
        .with_inner_size(LogicalSize::new(780.0, 520.0))
        .build(&event_loop)
        .expect("create window");
    let _webview = WebViewBuilder::new().with_html(html()).build(&window)?;
    println!("[ready]");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}
