//! M0 技术验证 #7：SQLite FTS5 trigram 在 10 万条中英混合数据上的搜索耗时。
//!
//! 运行：`cargo run -p spike-search-bench --release [-- 条数]`
//!
//! 测量内容：
//! - 批量写入耗时、数据库体积
//! - 单条增量写入耗时（模拟实时采集，含 FTS 索引更新）
//! - ≥3 字符查询：FTS5 trigram，按相关度 / 按时间倒序两种排序
//! - 1–2 字符查询：LIKE 扫描最近 N 条（trigram 不支持短于 3 个字符的查询）

use rusqlite::{Connection, params};
use std::time::{Duration, Instant};

const CJK_WORDS: &[&str] = &[
    "剪贴板",
    "历史记录",
    "同步",
    "设备",
    "局域网",
    "加密",
    "搜索",
    "粘贴",
    "队列",
    "快捷键",
    "会议",
    "周报",
    "地址",
    "验证码",
    "项目",
    "计划",
    "需求",
    "测试",
    "发布",
    "版本",
    "用户",
    "数据库",
    "性能",
    "内存",
    "窗口",
    "输入法",
    "中文",
    "图片",
    "文件",
    "链接",
    "代码",
    "今天",
    "明天",
    "下午",
    "上海市",
    "北京",
    "订单",
    "快递",
    "发票",
    "合同",
    "附件",
    "截图",
    "密码",
];
const EN_WORDS: &[&str] = &[
    "clipboard",
    "history",
    "sync",
    "device",
    "network",
    "encrypt",
    "search",
    "paste",
    "queue",
    "hotkey",
    "meeting",
    "report",
    "address",
    "project",
    "release",
    "version",
    "user",
    "database",
    "memory",
    "window",
    "image",
    "file",
    "link",
    "code",
    "rust",
    "async",
    "error",
    "config",
    "server",
    "client",
    "request",
    "response",
    "token",
    "build",
    "deploy",
    "commit",
    "branch",
];
const PUNCT: &[&str] = &["，", "。", "、", " ", ", ", ". ", ": ", "\n", " - ", "；"];

fn gen_text(rng: &mut fastrand::Rng) -> String {
    let kind = rng.u8(0..10);
    match kind {
        // 链接
        0 => format!(
            "https://{}.example.com/{}/{}?id={}",
            EN_WORDS[rng.usize(..EN_WORDS.len())],
            EN_WORDS[rng.usize(..EN_WORDS.len())],
            EN_WORDS[rng.usize(..EN_WORDS.len())],
            rng.u32(..)
        ),
        // 代码片段
        1 => format!(
            "fn {}_{}(x: i32) -> Result<{}, Error> {{ let v = x * {}; Ok(v) }}",
            EN_WORDS[rng.usize(..EN_WORDS.len())],
            EN_WORDS[rng.usize(..EN_WORDS.len())],
            EN_WORDS[rng.usize(..EN_WORDS.len())],
            rng.u32(..1000)
        ),
        // 中英混合长短文本
        _ => {
            let n = match rng.u8(0..10) {
                0..=5 => rng.usize(3..15),  // 短文本居多
                6..=8 => rng.usize(15..60), // 中等
                _ => rng.usize(60..300),    // 长文本
            };
            let mut s = String::new();
            for _ in 0..n {
                if rng.u8(0..10) < 6 {
                    s.push_str(CJK_WORDS[rng.usize(..CJK_WORDS.len())]);
                } else {
                    s.push_str(EN_WORDS[rng.usize(..EN_WORDS.len())]);
                }
                if rng.u8(0..3) == 0 {
                    s.push_str(PUNCT[rng.usize(..PUNCT.len())]);
                }
            }
            s
        }
    }
}

fn setup(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE clip_items (
             id         INTEGER PRIMARY KEY,
             content    TEXT NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE INDEX idx_clip_created ON clip_items(created_at DESC);
         CREATE VIRTUAL TABLE clip_fts USING fts5(
             content, content='clip_items', content_rowid='id', tokenize='trigram'
         );
         CREATE TRIGGER clip_ai AFTER INSERT ON clip_items BEGIN
             INSERT INTO clip_fts(rowid, content) VALUES (new.id, new.content);
         END;",
    )
}

struct Stats {
    samples: Vec<Duration>,
}

impl Stats {
    fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }
    fn push(&mut self, d: Duration) {
        self.samples.push(d);
    }
    fn report(&mut self, label: &str) {
        self.samples.sort();
        let n = self.samples.len();
        let pct = |p: f64| self.samples[((n as f64 * p) as usize).min(n - 1)];
        println!(
            "  {label:<34} n={n:<4} P50={:>8.3}ms  P95={:>8.3}ms  max={:>8.3}ms",
            ms(pct(0.50)),
            ms(pct(0.95)),
            ms(self.samples[n - 1])
        );
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn fts_phrase(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

fn main() -> rusqlite::Result<()> {
    let total: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    let dir = std::env::temp_dir().join("mypaste-search-bench");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let db_path = dir.join("bench.db");
    let conn = Connection::open(&db_path)?;
    setup(&conn)?;

    let sqlite_version: String = conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    println!(
        "SQLite {sqlite_version} · 数据量 {total} 条 · {}",
        db_path.display()
    );

    // ---------- 批量写入 ----------
    let mut rng = fastrand::Rng::with_seed(42);
    let t = Instant::now();
    {
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare("INSERT INTO clip_items(content, created_at) VALUES (?1, ?2)")?;
            for i in 0..total {
                stmt.execute(params![gen_text(&mut rng), i as i64])?;
            }
        }
        tx.commit()?;
    }
    let bulk = t.elapsed();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let size_mb = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0) as f64 / 1e6;
    let avg_len: f64 = conn.query_row("SELECT avg(length(content)) FROM clip_items", [], |r| {
        r.get(0)
    })?;
    println!(
        "\n[写入] 批量 {total} 条：{:.2}s（{:.1} 条/ms）· 平均长度 {avg_len:.0} 字符 · 库文件 {size_mb:.1}MB",
        bulk.as_secs_f64(),
        total as f64 / ms(bulk)
    );

    // ---------- 单条增量写入（模拟实时采集，每条一个事务） ----------
    let mut single = Stats::new();
    for i in 0..200 {
        let text = gen_text(&mut rng);
        let t = Instant::now();
        conn.execute(
            "INSERT INTO clip_items(content, created_at) VALUES (?1, ?2)",
            params![text, (total + i) as i64],
        )?;
        single.push(t.elapsed());
    }
    println!("\n[增量写入]（每条独立事务，含 FTS 索引更新）");
    single.report("单条 INSERT");

    // ---------- 查询 ----------
    let long_queries = [
        "剪贴板",
        "验证码",
        "局域网",
        "上海市",
        "历史记录",
        "数据库性能",
        "clipboard",
        "release",
        "example.com",
        "Result<",
        "同步 设备",
        "pas",
        "rus",
        "会议周报",
        "token",
        "不存在的词",
    ];
    let short_queries = ["剪", "同步", "密码", "rs", "a", "图"];

    let mut by_rank =
        conn.prepare("SELECT rowid FROM clip_fts WHERE clip_fts MATCH ?1 ORDER BY rank LIMIT 50")?;
    let mut by_time = conn.prepare(
        "SELECT c.id FROM clip_fts f JOIN clip_items c ON c.id = f.rowid
         WHERE clip_fts MATCH ?1 ORDER BY c.created_at DESC LIMIT 50",
    )?;
    let mut count_q = conn.prepare("SELECT count(*) FROM clip_fts WHERE clip_fts MATCH ?1")?;
    let mut like_recent = conn.prepare(
        "SELECT id FROM (SELECT id, content FROM clip_items ORDER BY created_at DESC LIMIT 5000)
         WHERE content LIKE ?1 LIMIT 50",
    )?;
    let mut like_all = conn.prepare(
        "SELECT id FROM clip_items WHERE content LIKE ?1 ORDER BY created_at DESC LIMIT 50",
    )?;

    // 预热
    for q in long_queries {
        let _: Vec<i64> = by_rank
            .query_map([fts_phrase(q)], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
    }

    println!("\n[逐条查询]（≥3 字符，FTS5 trigram；多词用空格分隔 = AND）");
    let mut s_rank = Stats::new();
    let mut s_time = Stats::new();
    for q in long_queries {
        // 空格分隔的多个词：每个词各自作为短语，隐式 AND
        let expr = q
            .split_whitespace()
            .map(fts_phrase)
            .collect::<Vec<_>>()
            .join(" ");
        let hits: i64 = count_q.query_row([&expr], |r| r.get(0))?;

        let mut best_rank = Duration::MAX;
        let mut best_time = Duration::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let _: Vec<i64> = by_rank
                .query_map([&expr], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let d = t.elapsed();
            s_rank.push(d);
            best_rank = best_rank.min(d);

            let t = Instant::now();
            let _: Vec<i64> = by_time
                .query_map([&expr], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let d = t.elapsed();
            s_time.push(d);
            best_time = best_time.min(d);
        }
        println!(
            "  {q:<14} 命中 {hits:>6} 条 · 相关度排序 {:>7.2}ms · 时间排序 {:>7.2}ms",
            ms(best_rank),
            ms(best_time)
        );
    }
    println!("\n[汇总]");
    s_rank.report("FTS 相关度排序 Top50");
    s_time.report("FTS 时间倒序 Top50");

    println!("\n[短查询]（1–2 字符，LIKE）");
    let mut s_like_recent = Stats::new();
    let mut s_like_all = Stats::new();
    for q in short_queries {
        let pat = format!("%{q}%");
        for _ in 0..5 {
            let t = Instant::now();
            let _: Vec<i64> = like_recent
                .query_map([&pat], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            s_like_recent.push(t.elapsed());

            let t = Instant::now();
            let _: Vec<i64> = like_all
                .query_map([&pat], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            s_like_all.push(t.elapsed());
        }
    }
    s_like_recent.report("LIKE 最近 5000 条");
    s_like_all.report("LIKE 全表（时间倒序 Top50）");

    // 最坏情况：全表 LIKE 搜一个不存在的短词，必须扫完整张表
    let t = Instant::now();
    let _: Vec<i64> = like_all
        .query_map(["%嗯%"], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    println!(
        "  LIKE 全表扫描（无命中，最坏情况）       {:>8.3}ms",
        ms(t.elapsed())
    );

    // ---------- 方案 B：rowid 即时间顺序 + 长短词混合查询 ----------
    // 让 rowid 单调代表"最近使用"（重新使用某条时为它分配新 rowid），
    // FTS5 就能按 rowid 倒序遍历倒排表，凑够 50 条即停，不必取出全部命中再排序。
    println!("\n[方案 B] rowid 倒序 + 长词走 FTS / 短词走 LIKE");
    let queries_b = [
        "剪贴板",
        "验证码",
        "历史记录",
        "clipboard",
        "release",
        "token",
        "数据库性能",
        "会议周报",
        "同步 设备",
        "剪贴板 同步",
        "rust 内存",
        "密码",
        "图",
        "不存在的词",
        "嗯嗯",
        "clipboard 嗯",
    ];
    let mut s_b = Stats::new();
    for q in queries_b {
        let mut best = Duration::MAX;
        let mut hits = 0;
        for _ in 0..5 {
            let t = Instant::now();
            let ids = planned_search(&conn, q)?;
            let d = t.elapsed();
            s_b.push(d);
            best = best.min(d);
            hits = ids.len();
        }
        println!("  {q:<14} Top50 返回 {hits:>2} 条 · {:>7.3}ms", ms(best));
    }
    s_b.report("方案 B 汇总");

    Ok(())
}

/// 搜索规划：空格分隔多个关键词（AND）。
/// - ≥3 字符的词：FTS5 trigram 短语
/// - 1–2 字符的词：LIKE（trigram 无法匹配短于 3 字符的词）
/// - 结果按 rowid 倒序（= 最近优先），取前 50 条
fn planned_search(conn: &Connection, query: &str) -> rusqlite::Result<Vec<i64>> {
    let (long, short): (Vec<&str>, Vec<&str>) = query
        .split_whitespace()
        .partition(|t| t.chars().count() >= 3);

    let mut params: Vec<String> = Vec::new();
    let like_clause = |col: &str, start: usize| {
        (0..short.len())
            .map(|i| format!(" AND {col} LIKE ?{} ESCAPE '\\'", start + i))
            .collect::<String>()
    };
    let escape_like = |s: &str| {
        format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    };

    let sql = if long.is_empty() {
        // 只有短词：沿 rowid 倒序扫描，命中 50 条即停
        params.extend(short.iter().map(|s| escape_like(s)));
        format!(
            "SELECT id FROM clip_items WHERE 1=1{} ORDER BY id DESC LIMIT 50",
            like_clause("content", 1)
        )
    } else {
        params.push(
            long.iter()
                .map(|t| fts_phrase(t))
                .collect::<Vec<_>>()
                .join(" "),
        );
        params.extend(short.iter().map(|s| escape_like(s)));
        if short.is_empty() {
            "SELECT rowid FROM clip_fts WHERE clip_fts MATCH ?1 ORDER BY rowid DESC LIMIT 50"
                .to_string()
        } else {
            format!(
                "SELECT c.id FROM clip_fts JOIN clip_items c ON c.id = clip_fts.rowid
                 WHERE clip_fts MATCH ?1{} ORDER BY clip_fts.rowid DESC LIMIT 50",
                like_clause("c.content", 2)
            )
        }
    };

    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| r.get(0))?;
    rows.collect()
}
