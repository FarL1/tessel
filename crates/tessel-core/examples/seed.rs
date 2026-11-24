//! 往数据目录里写入测试数据（开发、性能测试、截图用）。
//!
//! 用法：
//!   cargo run -p tessel-core --example seed --release -- <数据目录> [条数]   # 批量数据（性能测试）
//!   cargo run -p tessel-core --example seed --release -- <数据目录> --demo   # 一组好看的演示数据（截图用）

use tessel_core::{NewClip, Payload, RichFormats, Store};

fn text(s: &str, source: &str) -> NewClip {
    NewClip {
        payload: Payload::Text(s.into()),
        source: Some(source.into()),
        from_self: false,
        rich: RichFormats::default(),
    }
}

/// 演示数据：按"最旧 → 最新"的顺序写入，列表里最后写入的排在最前
fn demo(store: &mut Store) {
    let logo = include_bytes!("../../../assets/brand/png/tessel-512.png").to_vec();
    let items = vec![
        text("https://www.rust-lang.org/learn", "Microsoft Edge"),
        text(
            "SELECT id, content, source\nFROM clips\nWHERE kind = 'text'\nORDER BY seq DESC\nLIMIT 50;",
            "DataGrip",
        ),
        text("#10B981", "Figma"),
        NewClip {
            payload: Payload::Files(vec![
                r"C:\Windows\System32\drivers\etc\hosts".into(),
                r"C:\Windows\win.ini".into(),
            ]),
            source: Some("文件资源管理器".into()),
            from_self: false,
            rich: RichFormats::default(),
        },
        text(
            "会议纪要：周三下午 3 点评审 M2 范围。\n1. 粘贴队列与命令面板\n2. 图片 OCR，离线识别\n3. 公开发布：签名、自动更新",
            "飞书",
        ),
        text(
            "收货地址：上海市浦东新区世纪大道 100 号 32 层，联系人 李女士，电话 138 0000 0000",
            "钉钉",
        ),
        NewClip {
            payload: Payload::Image {
                png: logo,
                width: 512,
                height: 512,
            },
            source: Some("截图工具".into()),
            from_self: false,
            rich: RichFormats::default(),
        },
        text("#4F46E5", "Figma"),
        text(
            "fn main() -> anyhow::Result<()> {\n    let store = Store::open(&data_dir())?;\n    let clips = store.search(\"会议\", Filter::All, 50)?;\n    println!(\"找到 {} 条\", clips.len());\n    Ok(())\n}",
            "Visual Studio Code",
        ),
        text(
            "https://github.com/longbridge/gpui-component",
            "Google Chrome",
        ),
        // 动作菜单（Ctrl+K）的演示：格式化 JSON、去掉链接追踪参数
        text(
            r#"{"name":"Tessel","version":"0.2.0","features":["动作菜单","粘贴队列","OCR"],"sync":{"lan":true,"relay":null}}"#,
            "Postman",
        ),
        text(
            "https://www.bilibili.com/video/BV1GJ411x7h7?spm_id_from=333.1007.top_right_bar_window_history.content.click&vd_source=6f2a8c0d1e&t=42",
            "Microsoft Edge",
        ),
        NewClip {
            payload: Payload::Text("每一次复制，都拼进你的工作里。".into()),
            source: Some("Microsoft Word".into()),
            from_self: false,
            rich: RichFormats {
                html: Some(
                    "Version:0.9\r\n<b>每一次复制</b>，都拼进你的工作里。"
                        .as_bytes()
                        .to_vec(),
                ),
                rtf: None,
            },
        },
        text("验证码 482913，5 分钟内有效，请勿泄露给他人。", "微信"),
    ];
    for item in items {
        store.record(item).unwrap();
    }
    // 收藏两条，展示星标和"收藏"分类
    for clip in store.search("", tessel_core::Filter::All, 100).unwrap() {
        if clip.content.starts_with("收货地址") || clip.content.contains("rust-lang") {
            store.set_pinned(&clip.uuid, true).unwrap();
        }
    }
    println!("写入演示数据 {} 条", store.counts().unwrap().total);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("用法：seed <数据目录> [条数 | --demo]");
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(std::path::Path::new(&dir)).unwrap();

    let arg = args.next();
    if arg.as_deref() == Some("--demo") {
        demo(&mut store);
        return;
    }
    let n: usize = arg.map(|s| s.parse().expect("条数")).unwrap_or(3000);
    store.set_keep(n + 100);

    let sources = [
        "微信",
        "Google Chrome",
        "Visual Studio Code",
        "飞书",
        "记事本",
        "Figma",
    ];
    let samples = [
        "会议纪要：第 {i} 次周会，讨论同步协议与粘贴队列的优先级。",
        "https://example.com/articles/{i}?utm_source=tessel",
        "#{c}",
        "fn handler_{i}(req: Request) -> Response {{\n    let body = req.body();\n    Response::ok(body)\n}}",
        "订单号 TS{i:08}，预计明天送达，请保持电话畅通。",
        "SELECT * FROM orders WHERE id = {i} LIMIT 1;",
    ];
    let t0 = std::time::Instant::now();
    for i in 0..n {
        let template = samples[i % samples.len()];
        let content = template
            .replace("{i:08}", &format!("{i:08}"))
            .replace("{i}", &i.to_string())
            .replace("{c}", &format!("{:06X}", (i * 2_654_435_761) & 0xFFFFFF))
            .replace("{{", "{")
            .replace("}}", "}");
        store
            .record(text(&content, sources[i % sources.len()]))
            .unwrap();
    }
    println!("写入 {n} 条，用时 {:.1}s", t0.elapsed().as_secs_f64());
}
