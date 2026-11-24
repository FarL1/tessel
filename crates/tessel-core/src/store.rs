//! SQLite 存储与搜索。
//!
//! 设计要点（来自 M0 验证，见 docs/m0-report.md #7）：
//! - `seq`（rowid）就是"最近使用"顺序。再次复制同样的内容时，删掉旧行、用新的 seq 重新插入，
//!   uuid / 创建时间 / 收藏状态保持不变。这样 FTS5 可以按 rowid 倒序遍历，凑够条数就停。
//! - FTS5 trigram 只索引前 20000 个字符，避免超长文本让索引膨胀。
//! - 搜索：≥3 个字符的词走 FTS 短语，1–2 个字符的词走 LIKE（trigram 匹配不了短词）。
//! - 图片以内容哈希命名存成 PNG 文件，数据库里只存路径和尺寸。
//! - 同一个数据库可以被多个连接同时打开（UI 线程读、采集线程写），WAL 模式。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use crate::classify::classify_text;
use crate::model::{Clip, Filter, ImageInfo, Kind, NewClip, Payload, RichFormats};
use crate::time::now_ms;

pub type Result<T> = rusqlite::Result<T>;

/// 单条文本最多保存的字符数（再长的内容会被截断）
const MAX_TEXT_CHARS: usize = 1_000_000;
/// 写入多少条之后做一次清理
const PRUNE_EVERY: u32 = 50;
/// 默认最多保留的未收藏记录数
pub const DEFAULT_KEEP: usize = 5_000;

const SCHEMA_VERSION: i32 = 2;

/// v1 → v2：文本的富格式
const MIGRATE_V2: &str =
    "ALTER TABLE clips ADD COLUMN html BLOB; ALTER TABLE clips ADD COLUMN rtf BLOB;";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS clips (
    seq          INTEGER PRIMARY KEY,
    uuid         TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL,
    content      TEXT NOT NULL,
    hash         TEXT NOT NULL UNIQUE,
    source       TEXT,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    use_count    INTEGER NOT NULL DEFAULT 1,
    pinned       INTEGER NOT NULL DEFAULT 0,
    lang         TEXT,
    img_file     TEXT,
    img_w        INTEGER,
    img_h        INTEGER,
    img_bytes    INTEGER
);
CREATE INDEX IF NOT EXISTS idx_clips_kind ON clips(kind, seq DESC);
CREATE INDEX IF NOT EXISTS idx_clips_pinned ON clips(pinned, seq DESC);
CREATE VIRTUAL TABLE IF NOT EXISTS clip_fts USING fts5(
    content, content='clips', content_rowid='seq', tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS clips_ai AFTER INSERT ON clips BEGIN
    INSERT INTO clip_fts(rowid, content) VALUES (new.seq, substr(new.content, 1, 20000));
END;
CREATE TRIGGER IF NOT EXISTS clips_ad AFTER DELETE ON clips BEGIN
    INSERT INTO clip_fts(clip_fts, rowid, content)
    VALUES ('delete', old.seq, substr(old.content, 1, 20000));
END;
";

/// 列表查询只取内容的前 N 个字符（标题、文件数、颜色都够用），完整内容用 `get()` 按需读取
pub const LIST_CONTENT_CHARS: usize = 4000;

const LIST_COLUMNS: &str = "c.seq, c.uuid, c.kind, substr(c.content, 1, 4000), c.source,      c.created_at, c.last_used_at, c.use_count, c.pinned, c.lang, c.img_file, c.img_w, c.img_h, c.img_bytes, \n     (c.html IS NOT NULL OR c.rtf IS NOT NULL)";

const COLUMNS: &str = "c.seq, c.uuid, c.kind, c.content, c.source, c.created_at, c.last_used_at, \
     c.use_count, c.pinned, c.lang, c.img_file, c.img_w, c.img_h, c.img_bytes,      (c.html IS NOT NULL OR c.rtf IS NOT NULL)";

/// 各分类的条数（用于分类标签上的数字）
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub total: usize,
    pub pinned: usize,
    pub by_kind: [usize; 6],
}

impl Counts {
    pub fn get(&self, filter: Filter) -> usize {
        match filter {
            Filter::All => self.total,
            Filter::Pinned => self.pinned,
            Filter::Kind(k) => self.by_kind[Kind::ALL.iter().position(|x| *x == k).unwrap_or(0)],
        }
    }
}

pub struct Store {
    conn: Connection,
    images_dir: PathBuf,
    inserts_since_prune: u32,
    keep: usize,
}

impl Store {
    /// 打开（或创建）`dir/tessel.db`，图片存放在 `dir/images/`
    pub fn open(dir: &Path) -> Result<Self> {
        let images_dir = dir.join("images");
        let _ = std::fs::create_dir_all(&images_dir);
        let conn = Connection::open(dir.join("tessel.db"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 3000;
             PRAGMA foreign_keys = ON;",
        )?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            // 逐级迁移，放在一个事务里：中途失败不会留下半升级的数据库
            let mut sql = String::from("BEGIN;");
            if version < 1 {
                sql.push_str(SCHEMA);
            }
            if version < 2 {
                sql.push_str(MIGRATE_V2);
            }
            sql.push_str(&format!("PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"));
            conn.execute_batch(&sql)?;
        }
        Ok(Self {
            conn,
            images_dir,
            inserts_since_prune: 0,
            keep: DEFAULT_KEEP,
        })
    }

    pub fn set_keep(&mut self, keep: usize) {
        self.keep = keep;
    }

    /// 记录一条新采集的内容。内容相同（哈希相同）时不新增，而是把已有记录提到最前。
    /// 返回记录的 seq；空内容返回 `None`。
    pub fn record(&mut self, new: NewClip) -> Result<Option<i64>> {
        let now = now_ms();
        let rich = new.rich;
        let (kind, content, lang, image) = match new.payload {
            Payload::Text(text) => {
                if text.trim().is_empty() {
                    return Ok(None);
                }
                let text = if text.chars().count() > MAX_TEXT_CHARS {
                    text.chars().take(MAX_TEXT_CHARS).collect()
                } else {
                    text
                };
                let (kind, lang) = classify_text(&text);
                (kind, text, lang, None)
            }
            Payload::Files(files) => {
                if files.is_empty() {
                    return Ok(None);
                }
                (Kind::File, files.join("\n"), None, None)
            }
            Payload::Image { png, width, height } => (
                Kind::Image,
                format!("图片 {width}×{height}"),
                None,
                Some((png, width, height)),
            ),
        };

        let hash = match &image {
            Some((png, ..)) => blake3::hash(png).to_hex().to_string(),
            None => {
                let mut h = blake3::Hasher::new();
                h.update(kind.as_str().as_bytes());
                h.update(&[0]);
                h.update(content.as_bytes());
                h.finalize().to_hex().to_string()
            }
        };

        let tx = self.conn.transaction()?;
        let existing = tx
            .query_row(
                "SELECT uuid, created_at, use_count, pinned, source, img_file, img_w, img_h, img_bytes,
                        html, rtf
                 FROM clips WHERE hash = ?1",
                [&hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                        r.get::<_, Option<i64>>(8)?,
                        RichFormats {
                            html: r.get(9)?,
                            rtf: r.get(10)?,
                        },
                    ))
                },
            )
            .optional()?;

        let is_new = existing.is_none();
        let (uuid, created_at, use_count, pinned, source, img_file, img_w, img_h, img_bytes, rich) =
            match existing {
                Some((uuid, created, count, pinned, old_source, file, w, h, bytes, old_rich)) => {
                    tx.execute("DELETE FROM clips WHERE hash = ?1", [&hash])?;
                    let source = if new.from_self {
                        old_source
                    } else {
                        new.source.or(old_source)
                    };
                    // 这次没带富格式（比如"粘贴为纯文本"后被重新采集）时，保留之前的
                    let rich = if rich.is_empty() { old_rich } else { rich };
                    (
                        uuid,
                        created,
                        count + 1,
                        pinned,
                        source,
                        file,
                        w,
                        h,
                        bytes,
                        rich,
                    )
                }
                None => {
                    let (file, w, h, bytes) = match &image {
                        Some((png, w, h)) => {
                            let name = format!("{hash}.png");
                            let path = self.images_dir.join(&name);
                            if !path.exists() {
                                std::fs::write(&path, png).map_err(|e| {
                                    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
                                })?;
                            }
                            (
                                Some(name),
                                Some(*w as i64),
                                Some(*h as i64),
                                Some(png.len() as i64),
                            )
                        }
                        None => (None, None, None, None),
                    };
                    (
                        uuid::Uuid::now_v7().to_string(),
                        now,
                        1,
                        false,
                        if new.from_self { None } else { new.source },
                        file,
                        w,
                        h,
                        bytes,
                        rich,
                    )
                }
            };
        // 只有文本类才保存富格式
        let rich = if image.is_none() && kind != Kind::File {
            rich
        } else {
            RichFormats::default()
        };

        tx.execute(
            "INSERT INTO clips (uuid, kind, content, hash, source, created_at, last_used_at,
                                use_count, pinned, lang, img_file, img_w, img_h, img_bytes,
                                html, rtf)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                uuid,
                kind.as_str(),
                content,
                hash,
                source,
                created_at,
                now,
                use_count,
                pinned,
                lang,
                img_file,
                img_w,
                img_h,
                img_bytes,
                rich.html,
                rich.rtf
            ],
        )?;
        let seq = tx.last_insert_rowid();
        tx.commit()?;

        if is_new {
            self.inserts_since_prune += 1;
            if self.inserts_since_prune >= PRUNE_EVERY {
                self.inserts_since_prune = 0;
                self.prune(self.keep)?;
            }
        }
        Ok(Some(seq))
    }

    /// 读取一条记录的完整内容
    pub fn get(&self, uuid: &str) -> Result<Option<Clip>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM clips c WHERE c.uuid = ?1"),
                [uuid],
                |r| self.row_to_clip(r),
            )
            .optional()
    }

    /// 读取一条记录的富格式（列表和预览不需要，粘贴时才读）
    pub fn rich_formats(&self, uuid: &str) -> Result<RichFormats> {
        self.conn
            .query_row("SELECT html, rtf FROM clips WHERE uuid = ?1", [uuid], |r| {
                Ok(RichFormats {
                    html: r.get(0)?,
                    rtf: r.get(1)?,
                })
            })
            .optional()
            .map(Option::unwrap_or_default)
    }

    /// 清空所有未收藏的记录，返回删除的条数
    pub fn clear_unpinned(&mut self) -> Result<usize> {
        self.prune(0)
    }

    /// 搜索。空格分隔多个关键词（AND）；结果按最近使用倒序。
    /// 返回的 `content` 最多 `LIST_CONTENT_CHARS` 个字符。
    pub fn search(&self, query: &str, filter: Filter, limit: usize) -> Result<Vec<Clip>> {
        let (long, short): (Vec<&str>, Vec<&str>) = query
            .split_whitespace()
            .partition(|t| t.chars().count() >= 3);

        let mut conds: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        match filter {
            Filter::All => {}
            Filter::Kind(k) => {
                conds.push("c.kind = ?".into());
                args.push(k.as_str().into());
            }
            Filter::Pinned => conds.push("c.pinned = 1".into()),
        }
        for s in &short {
            conds.push("c.content LIKE ? ESCAPE '\\'".into());
            args.push(format!(
                "%{}%",
                s.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            ));
        }
        let where_extra: String = conds.iter().map(|c| format!(" AND {c}")).collect();

        let sql = if long.is_empty() {
            format!(
                "SELECT {LIST_COLUMNS} FROM clips c WHERE 1=1{where_extra} ORDER BY c.seq DESC LIMIT {limit}"
            )
        } else {
            let fts = long
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" ");
            args.insert(0, fts);
            format!(
                "SELECT {LIST_COLUMNS} FROM clip_fts JOIN clips c ON c.seq = clip_fts.rowid
                 WHERE clip_fts MATCH ?{where_extra} ORDER BY clip_fts.rowid DESC LIMIT {limit}"
            )
        };

        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| self.row_to_clip(r))?;
        rows.collect()
    }

    pub fn counts(&self) -> Result<Counts> {
        let mut counts = Counts::default();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT kind, count(*) FROM clips GROUP BY kind")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (kind, n) = row?;
            let kind = Kind::parse(&kind);
            let ix = Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
            counts.by_kind[ix] += n as usize;
            counts.total += n as usize;
        }
        counts.pinned =
            self.conn
                .query_row("SELECT count(*) FROM clips WHERE pinned = 1", [], |r| {
                    r.get::<_, i64>(0)
                })? as usize;
        Ok(counts)
    }

    pub fn set_pinned(&self, uuid: &str, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE clips SET pinned = ?1 WHERE uuid = ?2",
            params![pinned, uuid],
        )?;
        Ok(())
    }

    pub fn delete(&self, uuid: &str) -> Result<()> {
        let file: Option<Option<String>> = self
            .conn
            .query_row("SELECT img_file FROM clips WHERE uuid = ?1", [uuid], |r| {
                r.get(0)
            })
            .optional()?;
        self.conn
            .execute("DELETE FROM clips WHERE uuid = ?1", [uuid])?;
        if let Some(Some(name)) = file {
            let _ = std::fs::remove_file(self.images_dir.join(name));
        }
        Ok(())
    }

    /// 只保留最近的 `keep` 条未收藏记录（收藏的永远保留）
    pub fn prune(&mut self, keep: usize) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "DELETE FROM clips WHERE pinned = 0 AND seq NOT IN
               (SELECT seq FROM clips WHERE pinned = 0 ORDER BY seq DESC LIMIT ?1)
             RETURNING img_file",
        )?;
        let files: Vec<Option<String>> = stmt
            .query_map([keep as i64], |r| r.get(0))?
            .collect::<Result<_>>()?;
        for name in files.iter().flatten() {
            let _ = std::fs::remove_file(self.images_dir.join(name));
        }
        Ok(files.len())
    }

    fn row_to_clip(&self, r: &Row) -> Result<Clip> {
        let img_file: Option<String> = r.get(10)?;
        let image = match img_file {
            Some(name) => Some(ImageInfo {
                path: self.images_dir.join(name),
                width: r.get::<_, Option<i64>>(11)?.unwrap_or(0) as u32,
                height: r.get::<_, Option<i64>>(12)?.unwrap_or(0) as u32,
                bytes: r.get::<_, Option<i64>>(13)?.unwrap_or(0) as u64,
            }),
            None => None,
        };
        Ok(Clip {
            seq: r.get(0)?,
            uuid: r.get(1)?,
            kind: Kind::parse(&r.get::<_, String>(2)?),
            content: r.get(3)?,
            source: r.get(4)?,
            created_at: r.get(5)?,
            last_used_at: r.get(6)?,
            use_count: r.get(7)?,
            pinned: r.get(8)?,
            lang: r.get(9)?,
            image,
            has_rich: r.get(14)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "tessel-test-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Store::open(&dir).unwrap()
    }

    fn text(s: &str, source: &str) -> NewClip {
        NewClip {
            payload: Payload::Text(s.into()),
            source: Some(source.into()),
            from_self: false,
            rich: Default::default(),
        }
    }

    #[test]
    fn record_dedup_and_bump() {
        let mut store = temp_store("dedup");
        let a = store.record(text("第一条内容", "记事本")).unwrap().unwrap();
        let b = store.record(text("第二条内容", "微信")).unwrap().unwrap();
        assert!(b > a);
        // 再次复制第一条：不新增，提到最前，使用次数 +1，来源更新
        let a2 = store.record(text("第一条内容", "Chrome")).unwrap().unwrap();
        assert!(a2 > b);
        let all = store.search("", Filter::All, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].content, "第一条内容");
        assert_eq!(all[0].use_count, 2);
        assert_eq!(all[0].source.as_deref(), Some("Chrome"));
        // Tessel 自己写入的不改来源
        store
            .record(NewClip {
                payload: Payload::Text("第二条内容".into()),
                source: Some("Tessel".into()),
                from_self: true,
                rich: Default::default(),
            })
            .unwrap();
        let all = store.search("", Filter::All, 10).unwrap();
        assert_eq!(all[0].content, "第二条内容");
        assert_eq!(all[0].source.as_deref(), Some("微信"));
        // 空白内容不记录
        assert_eq!(store.record(text("   \n ", "x")).unwrap(), None);
    }

    #[test]
    fn search_long_short_and_filters() {
        let mut store = temp_store("search");
        store
            .record(text("剪贴板历史记录支持全文搜索", "a"))
            .unwrap();
        store.record(text("多设备同步：局域网直连", "b")).unwrap();
        store
            .record(text("https://github.com/tessel", "c"))
            .unwrap();
        store.record(text("#4F6BED", "d")).unwrap();

        // ≥3 字符：FTS
        assert_eq!(store.search("剪贴板", Filter::All, 10).unwrap().len(), 1);
        // 2 字符：LIKE（trigram 匹配不了）
        assert_eq!(store.search("同步", Filter::All, 10).unwrap().len(), 1);
        // 长短混合
        assert_eq!(
            store.search("局域网 同步", Filter::All, 10).unwrap().len(),
            1
        );
        // 大小写不敏感
        assert_eq!(store.search("GITHUB", Filter::All, 10).unwrap().len(), 1);
        // 分类筛选
        assert_eq!(
            store
                .search("", Filter::Kind(Kind::Link), 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search("", Filter::Kind(Kind::Color), 10)
                .unwrap()
                .len(),
            1
        );
        // LIKE 通配符要转义
        assert_eq!(store.search("%", Filter::All, 10).unwrap().len(), 0);

        let counts = store.counts().unwrap();
        assert_eq!(counts.total, 4);
        assert_eq!(counts.get(Filter::Kind(Kind::Text)), 2);
    }

    #[test]
    fn pin_delete_prune() {
        let mut store = temp_store("prune");
        for i in 0..10 {
            store.record(text(&format!("内容 {i}"), "x")).unwrap();
        }
        let all = store.search("", Filter::All, 100).unwrap();
        let oldest = all.last().unwrap().uuid.clone();
        store.set_pinned(&oldest, true).unwrap();
        assert_eq!(store.search("", Filter::Pinned, 10).unwrap().len(), 1);

        // 只保留 3 条未收藏，收藏的那条也要留下
        let removed = store.prune(3).unwrap();
        assert_eq!(removed, 6);
        let left = store.search("", Filter::All, 100).unwrap();
        assert_eq!(left.len(), 4);
        assert!(left.iter().any(|c| c.uuid == oldest));

        // 列表查询截断内容，get() 返回完整内容
        let long = "长".repeat(LIST_CONTENT_CHARS + 100);
        store.record(text(&long, "x")).unwrap();
        let listed = &store.search("", Filter::All, 1).unwrap()[0];
        assert_eq!(listed.content.chars().count(), LIST_CONTENT_CHARS);
        assert_eq!(store.get(&listed.uuid).unwrap().unwrap().content, long);
        let long_uuid = listed.uuid.clone();
        store.delete(&long_uuid).unwrap();

        store.delete(&oldest).unwrap();
        assert_eq!(store.search("", Filter::All, 100).unwrap().len(), 3);
        // 删除后 FTS 索引也要同步
        assert_eq!(store.search("内容", Filter::All, 100).unwrap().len(), 3);
        store.set_pinned(&left[0].uuid, true).unwrap();
        assert_eq!(store.clear_unpinned().unwrap(), 2);
        assert_eq!(store.search("", Filter::All, 100).unwrap().len(), 1);
    }

    #[test]
    fn images_are_stored_as_files() {
        let mut store = temp_store("image");
        let png = vec![1u8, 2, 3, 4];
        store
            .record(NewClip {
                payload: Payload::Image {
                    png: png.clone(),
                    width: 2,
                    height: 1,
                },
                source: None,
                from_self: false,
                rich: Default::default(),
            })
            .unwrap();
        let clip = &store.search("", Filter::Kind(Kind::Image), 10).unwrap()[0];
        let info = clip.image.as_ref().unwrap();
        assert_eq!(std::fs::read(&info.path).unwrap(), png);
        store.delete(&clip.uuid).unwrap();
        assert!(!info.path.exists());
    }

    #[test]
    fn rich_formats_kept_when_recaptured_plain() {
        let mut store = temp_store("rich");
        let rich = RichFormats {
            html: Some("Version:0.9\r\n<b>加粗</b>".as_bytes().to_vec()),
            rtf: Some(br"{\rtf1 {\b bold}}".to_vec()),
        };
        store
            .record(NewClip {
                payload: Payload::Text("加粗".into()),
                source: Some("Word".into()),
                from_self: false,
                rich: rich.clone(),
            })
            .unwrap();
        let clip = &store.search("", Filter::All, 1).unwrap()[0];
        assert!(clip.has_rich);
        assert_eq!(store.rich_formats(&clip.uuid).unwrap(), rich);

        // "粘贴为纯文本"后被重新采集：不带富格式，旧的富格式要保留
        store.record(text("加粗", "Tessel")).unwrap();
        let clip = &store.search("", Filter::All, 1).unwrap()[0];
        assert!(clip.has_rich);
        assert_eq!(store.rich_formats(&clip.uuid).unwrap(), rich);

        // 纯文本记录没有富格式
        store.record(text("普通文本", "记事本")).unwrap();
        let clip = &store.search("普通文本", Filter::All, 1).unwrap()[0];
        assert!(!clip.has_rich);
        assert!(store.rich_formats(&clip.uuid).unwrap().is_empty());
    }

    #[test]
    fn migrates_v1_database() {
        let dir = std::env::temp_dir().join(format!(
            "tessel-test-migrate-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        {
            // 造一个 v1 数据库（只有 v1 的表结构）并写入一条记录
            let conn = Connection::open(dir.join("tessel.db")).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch("PRAGMA user_version = 1").unwrap();
            conn.execute(
                "INSERT INTO clips (uuid, kind, content, hash, created_at, last_used_at)
                 VALUES ('u1', 'text', '旧版本的记录', 'h1', 1, 1)",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        let version: i32 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let clips = store.search("旧版本", Filter::All, 10).unwrap();
        assert_eq!(clips.len(), 1);
        assert!(!clips[0].has_rich);
    }
}
