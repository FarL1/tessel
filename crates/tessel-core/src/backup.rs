//! 导入导出：备份、换电脑、分享片段库。
//!
//! 文件格式是 JSON Lines（`.jsonl`）：第一行是文件头，之后每行一条记录，按最近使用从旧到新排列。
//! 图片和富格式（HTML / RTF）用 base64 内嵌，一个文件就是完整的备份。逐行读写，文件再大也不会
//! 一次性读进内存。
//!
//! 导入时内容相同（哈希相同）的记录合并：收藏、片段标记取并集，没有备注的补上备注；
//! 新记录排在现有记录的后面（当作更早的历史），彼此之间保持原来的先后顺序。

use std::io::{BufRead, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::classify::classify_text;
use crate::i18n::tr;
use crate::model::Kind;
use crate::pinyin_index;
use crate::store::{MAX_TEXT_CHARS, Store, image_hash, text_hash};
use crate::time::now_ms;
use crate::trf;

pub const FORMAT: &str = "tessel-export";
pub const VERSION: u32 = 1;

/// 每个事务导入多少条：分批提交，导入期间采集线程照常写入
const BATCH: usize = 200;

#[derive(Serialize, Deserialize)]
struct Header {
    format: String,
    version: u32,
    #[serde(default)]
    app_version: String,
    #[serde(default)]
    exported_at: i64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn one() -> i64 {
    1
}

#[derive(Serialize, Deserialize)]
struct Record {
    uuid: String,
    kind: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    created_at: i64,
    last_used_at: i64,
    #[serde(default = "one")]
    use_count: i64,
    #[serde(default, skip_serializing_if = "is_false")]
    pinned: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    snippet: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
    /// base64
    #[serde(default, skip_serializing_if = "Option::is_none")]
    html: Option<String>,
    /// base64
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rtf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image: Option<ImageRecord>,
}

#[derive(Serialize, Deserialize)]
struct ImageRecord {
    /// base64 编码的 PNG
    png: String,
    width: u32,
    height: u32,
}

/// 导出哪些记录
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportScope {
    All,
    /// 只导出收藏和片段（分享片段库、只备份重要内容）
    Kept,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub merged: usize,
    /// 无法识别、内容损坏的行
    pub skipped: usize,
}

impl Store {
    /// 导出到 `out`，返回导出的条数。图片文件丢失的记录跳过。
    pub fn export(
        &self,
        scope: ExportScope,
        app_version: &str,
        out: &mut impl Write,
    ) -> Result<usize, String> {
        let io = |e: std::io::Error| trf!("写入文件失败：{e}", "Could not write the file: {e}");
        let db =
            |e: rusqlite::Error| trf!("读取数据库失败：{e}", "Could not read the database: {e}");
        let header = Header {
            format: FORMAT.into(),
            version: VERSION,
            app_version: app_version.into(),
            exported_at: now_ms(),
        };
        writeln!(
            out,
            "{}",
            serde_json::to_string(&header).unwrap_or_default()
        )
        .map_err(io)?;

        let filter = match scope {
            ExportScope::All => "",
            ExportScope::Kept => "WHERE pinned = 1 OR snippet = 1",
        };
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT uuid, kind, content, source, created_at, last_used_at, use_count, pinned,
                        snippet, note, lang, html, rtf, img_file, img_w, img_h
                 FROM clips {filter} ORDER BY seq ASC"
            ))
            .map_err(db)?;
        let mut rows = stmt.query([]).map_err(db)?;
        let mut count = 0;
        while let Some(r) = rows.next().map_err(db)? {
            let bytes = |i: usize| -> rusqlite::Result<Option<String>> {
                Ok(r.get::<_, Option<Vec<u8>>>(i)?.map(|b| B64.encode(b)))
            };
            let image = match r.get::<_, Option<String>>(13).map_err(db)? {
                Some(name) => match std::fs::read(self.images_dir.join(name)) {
                    Ok(png) => Some(ImageRecord {
                        png: B64.encode(png),
                        width: r.get::<_, Option<u32>>(14).map_err(db)?.unwrap_or(0),
                        height: r.get::<_, Option<u32>>(15).map_err(db)?.unwrap_or(0),
                    }),
                    // 图片文件被删掉了：这条导不出来
                    Err(_) => continue,
                },
                None => None,
            };
            let record = Record {
                uuid: r.get(0).map_err(db)?,
                kind: r.get(1).map_err(db)?,
                content: r.get(2).map_err(db)?,
                source: r.get(3).map_err(db)?,
                created_at: r.get(4).map_err(db)?,
                last_used_at: r.get(5).map_err(db)?,
                use_count: r.get(6).map_err(db)?,
                pinned: r.get(7).map_err(db)?,
                snippet: r.get(8).map_err(db)?,
                note: r.get(9).map_err(db)?,
                lang: r.get(10).map_err(db)?,
                html: bytes(11).map_err(db)?,
                rtf: bytes(12).map_err(db)?,
                image,
            };
            let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
            writeln!(out, "{line}").map_err(io)?;
            count += 1;
        }
        out.flush().map_err(io)?;
        Ok(count)
    }

    /// 从 `input` 导入
    pub fn import(&mut self, input: impl BufRead) -> Result<ImportReport, String> {
        let db =
            |e: rusqlite::Error| trf!("写入数据库失败：{e}", "Could not write the database: {e}");
        let mut lines = input.lines();
        let header_line = loop {
            match lines.next() {
                Some(Ok(line)) if line.trim().is_empty() => continue,
                Some(Ok(line)) => break line,
                Some(Err(e)) => {
                    return Err(trf!("读取文件失败：{e}", "Could not read the file: {e}"));
                }
                None => return Err(tr("文件是空的", "The file is empty").into()),
            }
        };
        let header: Header = serde_json::from_str(header_line.trim_start_matches('\u{feff}'))
            .ok()
            .filter(|h: &Header| h.format == FORMAT)
            .ok_or_else(|| {
                tr(
                    "这不是 Tessel 导出的文件",
                    "This isn't a file exported by Tessel",
                )
                .to_string()
            })?;
        if header.version > VERSION {
            return Err(tr(
                "这个文件来自更新版本的 Tessel，请先升级",
                "This file comes from a newer version of Tessel; please update first",
            )
            .into());
        }

        // 新记录排在现有记录后面：seq 从远低于现有最小值的位置开始，按文件顺序递增
        let min_seq: i64 = self
            .conn
            .query_row("SELECT coalesce(min(seq), 0) FROM clips", [], |r| r.get(0))
            .map_err(db)?;
        let mut next_seq = min_seq - 1_000_000_000;

        let mut report = ImportReport::default();
        let mut tx = self.conn.transaction().map_err(db)?;
        let mut in_batch = 0;
        for line in lines {
            let Ok(line) = line else {
                report.skipped += 1;
                continue;
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Record>(&line) else {
                report.skipped += 1;
                continue;
            };
            match import_one(&tx, &self.images_dir, record, next_seq).map_err(db)? {
                Some(true) => {
                    report.added += 1;
                    next_seq += 1;
                }
                Some(false) => report.merged += 1,
                None => report.skipped += 1,
            }
            in_batch += 1;
            if in_batch >= BATCH {
                tx.commit().map_err(db)?;
                tx = self.conn.transaction().map_err(db)?;
                in_batch = 0;
            }
        }
        tx.commit().map_err(db)?;
        Ok(report)
    }
}

/// 导入一条：`Some(true)` 新增，`Some(false)` 合并到已有记录，`None` 内容无效
fn import_one(
    tx: &rusqlite::Transaction,
    images_dir: &std::path::Path,
    r: Record,
    seq: i64,
) -> rusqlite::Result<Option<bool>> {
    let decode = |s: &Option<String>| s.as_ref().and_then(|s| B64.decode(s).ok());
    let png = match &r.image {
        Some(img) => match B64.decode(&img.png) {
            Ok(png) => Some(png),
            Err(_) => return Ok(None),
        },
        None => None,
    };
    let (kind, content, lang) = match &png {
        Some(_) => (Kind::Image, r.content.clone(), None),
        None => {
            let content: String = r.content.chars().take(MAX_TEXT_CHARS).collect();
            if content.trim().is_empty() {
                return Ok(None);
            }
            // 文件按原类型保存；文本类以本机的识别结果为准（识别规则可能变过）
            if Kind::parse(&r.kind) == Kind::File {
                (Kind::File, content, None)
            } else {
                let (kind, lang) = classify_text(&content);
                (kind, content, lang.or(r.lang.clone()))
            }
        }
    };
    let hash = match &png {
        Some(png) => image_hash(png),
        None => text_hash(kind, &content),
    };

    // 已经有同样的内容：合并标记和备注
    let merged = tx.execute(
        "UPDATE clips SET pinned = pinned OR ?1, snippet = snippet OR ?2,
                          note = coalesce(note, ?3)
         WHERE hash = ?4",
        params![r.pinned, r.snippet, r.note, hash],
    )?;
    if merged > 0 {
        return Ok(Some(false));
    }

    let (img_file, img_w, img_h, img_bytes) = match (&png, &r.image) {
        (Some(png), Some(img)) => {
            let name = format!("{hash}.png");
            let path = images_dir.join(&name);
            if !path.exists() {
                std::fs::write(&path, png)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            }
            (
                Some(name),
                Some(img.width),
                Some(img.height),
                Some(png.len() as i64),
            )
        }
        _ => (None, None, None, None),
    };
    // uuid 撞上另一条不同内容的记录（极少见）：换一个新的
    let taken: Option<i64> = tx
        .query_row("SELECT 1 FROM clips WHERE uuid = ?1", [&r.uuid], |row| {
            row.get(0)
        })
        .optional()?;
    let uuid = match (taken, uuid::Uuid::parse_str(&r.uuid)) {
        (None, Ok(_)) => r.uuid.clone(),
        _ => uuid::Uuid::now_v7().to_string(),
    };
    let py = match &png {
        Some(_) => None,
        None => pinyin_index::index_text(&content),
    };
    tx.execute(
        "INSERT INTO clips (seq, uuid, kind, content, hash, source, created_at, last_used_at,
                            use_count, pinned, lang, img_file, img_w, img_h, img_bytes,
                            html, rtf, note, py, snippet)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20)",
        params![
            seq,
            uuid,
            kind.as_str(),
            content,
            hash,
            r.source,
            r.created_at,
            r.last_used_at,
            r.use_count.max(1),
            r.pinned,
            lang,
            img_file,
            img_w,
            img_h,
            img_bytes,
            // 只有文本类保存富格式
            if png.is_none() && kind != Kind::File {
                decode(&r.html)
            } else {
                None
            },
            if png.is_none() && kind != Kind::File {
                decode(&r.rtf)
            } else {
                None
            },
            r.note,
            py,
            r.snippet
        ],
    )?;
    Ok(Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Filter, NewClip, Payload, RichFormats};

    struct Temp(std::path::PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tessel-test-backup-{name}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn text(s: &str) -> NewClip {
        NewClip {
            payload: Payload::Text(s.into()),
            source: Some("记事本".into()),
            from_self: false,
            rich: RichFormats::default(),
        }
    }

    #[test]
    fn roundtrip_merge_and_scope() {
        let (a, b) = (Temp::new("a"), Temp::new("b"));
        let mut src = Store::open(&a.0).unwrap();
        src.record(text("最早的一条")).unwrap();
        src.record(NewClip {
            payload: Payload::Text("加粗".into()),
            source: Some("Word".into()),
            from_self: false,
            rich: RichFormats {
                html: Some(b"<b>x</b>".to_vec()),
                rtf: None,
            },
        })
        .unwrap();
        src.record(NewClip {
            payload: Payload::Image {
                png: vec![9, 8, 7],
                width: 3,
                height: 1,
            },
            source: None,
            from_self: false,
            rich: RichFormats::default(),
        })
        .unwrap();
        let snippet = src.create_snippet("今天是 {date}").unwrap().unwrap();
        src.set_note(&snippet, "日报开头 #模板").unwrap();
        let first = src.search("最早", Filter::All, 1).unwrap()[0].uuid.clone();
        src.set_pinned(&first, true).unwrap();

        let mut file = Vec::new();
        assert_eq!(src.export(ExportScope::All, "0.1.3", &mut file).unwrap(), 4);
        let mut kept = Vec::new();
        assert_eq!(
            src.export(ExportScope::Kept, "0.1.3", &mut kept).unwrap(),
            2
        );

        // 导入到另一台电脑（已经有一条记录）
        let mut dst = Store::open(&b.0).unwrap();
        dst.record(text("新电脑上的记录")).unwrap();
        dst.record(text("今天是 {date}")).unwrap();
        let report = dst.import(file.as_slice()).unwrap();
        assert_eq!(
            report,
            ImportReport {
                added: 3,
                merged: 1,
                skipped: 0
            }
        );
        let all = dst.search("", Filter::All, 10).unwrap();
        assert_eq!(all.len(), 5);
        // 新电脑上原有的记录在最前面，导入的排在后面并保持原来的先后
        assert_eq!(all[0].content, "今天是 {date}");
        assert_eq!(all[1].content, "新电脑上的记录");
        assert_eq!(all.last().unwrap().content, "最早的一条");
        // 合并：片段标记、备注补上
        let merged = &all[0];
        assert!(merged.snippet);
        assert_eq!(merged.note.as_deref(), Some("日报开头 #模板"));
        // 收藏、富格式、图片、uuid
        let pinned = dst.get(&first).unwrap().unwrap();
        assert!(pinned.pinned);
        let rich = dst.search("加粗", Filter::All, 1).unwrap()[0].clone();
        assert_eq!(
            dst.rich_formats(&rich.uuid).unwrap().html.as_deref(),
            Some(&b"<b>x</b>"[..])
        );
        let img = dst.search("", Filter::Kind(Kind::Image), 1).unwrap()[0].clone();
        assert_eq!(
            std::fs::read(&img.image.unwrap().path).unwrap(),
            vec![9, 8, 7]
        );
        // 拼音、备注搜索可用
        assert_eq!(dst.search("jts", Filter::All, 10).unwrap().len(), 1);
        assert_eq!(dst.search("#模板", Filter::All, 10).unwrap().len(), 1);

        // 再导入一次：全部合并，不重复
        let again = dst.import(file.as_slice()).unwrap();
        assert_eq!((again.added, again.merged), (0, 4));
        assert_eq!(dst.search("", Filter::All, 10).unwrap().len(), 5);
        // 新采集的内容照常排在最前
        dst.record(text("导入之后复制的")).unwrap();
        assert_eq!(
            dst.search("", Filter::All, 1).unwrap()[0].content,
            "导入之后复制的"
        );
    }

    #[test]
    fn bad_files() {
        let t = Temp::new("bad");
        let mut store = Store::open(&t.0).unwrap();
        assert!(store.import(&b""[..]).is_err());
        assert!(store.import(&b"{\"hello\":1}\n"[..]).is_err());
        let newer = format!("{{\"format\":\"{FORMAT}\",\"version\":99}}\n");
        assert!(store.import(newer.as_bytes()).is_err());
        // 坏掉的行跳过，其余照常导入
        let file = format!(
            "{{\"format\":\"{FORMAT}\",\"version\":1}}\nnot json\n{}\n",
            r#"{"uuid":"x","kind":"text","content":"好的一条","created_at":1,"last_used_at":1}"#
        );
        let report = store.import(file.as_bytes()).unwrap();
        assert_eq!((report.added, report.skipped), (1, 1));
    }
}
