//! 往数据目录里写入一批测试数据（开发和性能测试用）。
//!
//! 用法：cargo run -p tessel-core --example seed --release -- <数据目录> [条数]

use tessel_core::{NewClip, Payload, Store};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("用法：seed <数据目录> [条数]");
    let n: usize = args
        .next()
        .map(|s| s.parse().expect("条数"))
        .unwrap_or(3000);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(std::path::Path::new(&dir)).unwrap();
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
        let text = template
            .replace("{i:08}", &format!("{i:08}"))
            .replace("{i}", &i.to_string())
            .replace("{c}", &format!("{:06X}", (i * 2_654_435_761) & 0xFFFFFF))
            .replace("{{", "{")
            .replace("}}", "}");
        store
            .record(NewClip {
                payload: Payload::Text(text),
                source: Some(sources[i % sources.len()].into()),
                from_self: false,
            })
            .unwrap();
    }
    println!("写入 {n} 条，用时 {:.1}s", t0.elapsed().as_secs_f64());
}
