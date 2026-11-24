//! Tessel 命令行：在脚本、启动器、AutoHotkey 里使用剪贴板历史。
//!
//! 直接读写和桌面端同一个数据库（SQLite WAL，可以同时打开），所以不需要 Tessel 正在运行。
//! 数据目录和桌面端一致：`TESSEL_DATA_DIR`，默认 `%LOCALAPPDATA%\Tessel`。
//!
//! ```text
//! tessel-cli search 会议 -n 5          # 搜索（支持 app: type: is: #标签 -排除、拼音）
//! tessel-cli get 1                     # 输出最新一条的完整内容
//! tessel-cli copy --snippet 签名       # 把备注里带"签名"的片段（展开变量后）放到剪贴板
//! echo hello | tessel-cli add          # 加入历史
//! tessel-cli export backup.jsonl       # 导出（--kept 只导出收藏和片段）
//! ```

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use tessel_clipboard::WriteContent;
use tessel_core::backup::ExportScope;
use tessel_core::i18n::{self, tr};
use tessel_core::time::{now_ms, relative};
use tessel_core::{
    Clip, Config, Filter, Kind, LanguagePref, NewClip, Payload, Store, template, trf,
};

#[derive(Parser)]
#[command(
    name = "tessel-cli",
    version,
    about = "Tessel · 拼贴 clipboard history from the command line / 命令行使用剪贴板历史"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search the history (same syntax as the popup) / 搜索历史
    Search {
        /// Keywords and filters: app:chrome type:link is:pinned #tag -exclude, pinyin initials
        query: Vec<String>,
        /// How many results / 最多显示几条
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        /// Output JSON / 输出 JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the full content of an item / 输出一条记录的完整内容
    Get(Target),
    /// Put an item on the clipboard (snippets are expanded) / 把一条记录放到剪贴板
    Copy {
        #[command(flatten)]
        target: Target,
        /// Plain text only, drop formatting / 只要纯文本
        #[arg(long)]
        plain: bool,
    },
    /// Add text to the history (reads stdin when TEXT is omitted or "-") / 加入历史
    Add {
        text: Option<String>,
        /// Save it as a snippet (variables are expanded on paste) / 存为片段
        #[arg(long)]
        snippet: bool,
        /// Note, may contain #tags / 备注，可以带 #标签
        #[arg(long)]
        note: Option<String>,
    },
    /// List snippets / 列出片段
    Snippets {
        #[arg(long)]
        json: bool,
    },
    /// Export to a .jsonl file / 导出
    Export {
        file: PathBuf,
        /// Only pinned items and snippets / 只导出收藏和片段
        #[arg(long)]
        kept: bool,
    },
    /// Import a file exported by Tessel / 导入
    Import { file: PathBuf },
}

#[derive(Args)]
struct Target {
    /// Position in the history (1 = newest) or the item's uuid / 序号（1 是最新的）或 uuid
    #[arg(required_unless_present = "snippet")]
    item: Option<String>,
    /// A snippet whose note contains this text / 备注里带这段文字的片段
    #[arg(long, conflicts_with = "item")]
    snippet: Option<String>,
}

#[derive(Serialize)]
struct Item<'a> {
    position: usize,
    uuid: &'a str,
    kind: &'a str,
    title: String,
    note: Option<&'a str>,
    source: Option<&'a str>,
    pinned: bool,
    snippet: bool,
    last_used_at: i64,
}

fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TESSEL_DATA_DIR") {
        return PathBuf::from(dir);
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Tessel")
}

#[cfg(windows)]
fn system_is_chinese() -> bool {
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    lang & 0x3FF == 0x04
}

#[cfg(not(windows))]
fn system_is_chinese() -> bool {
    false
}

#[cfg(windows)]
fn local_time() -> template::LocalTime {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    template::LocalTime {
        year: t.wYear,
        month: t.wMonth as u8,
        day: t.wDay as u8,
        hour: t.wHour as u8,
        minute: t.wMinute as u8,
        weekday: t.wDayOfWeek as u8,
    }
}

#[cfg(not(windows))]
fn local_time() -> template::LocalTime {
    template::LocalTime::default()
}

/// 界面语言跟桌面端一致。不创建配置文件：只在已经存在时读取
fn apply_language(dir: &std::path::Path) {
    let pref = std::fs::read_to_string(Config::path(dir))
        .ok()
        .and_then(|text| Config::parse(&text).ok())
        .map(|c| c.language)
        .unwrap_or(LanguagePref::System);
    i18n::set_english(pref.resolve(system_is_chinese()));
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let dir = data_dir();
    apply_language(&dir);
    match run(cli.command, &dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tessel-cli: {err}");
            ExitCode::FAILURE
        }
    }
}

fn open(dir: &std::path::Path) -> Result<Store, String> {
    if !dir.join("tessel.db").exists() {
        return Err(trf!(
            "找不到剪贴板历史：{}（Tessel 还没运行过？）",
            "No clipboard history at {} (has Tessel been run yet?)",
            dir.display()
        ));
    }
    Store::open(dir).map_err(|e| trf!("无法打开数据库：{e}", "Could not open the database: {e}"))
}

fn run(command: Command, dir: &std::path::Path) -> Result<(), String> {
    match command {
        Command::Search { query, limit, json } => {
            let store = open(dir)?;
            let items = store
                .search(&query.join(" "), Filter::All, limit)
                .map_err(|e| e.to_string())?;
            print_items(&items, json)
        }
        Command::Snippets { json } => {
            let store = open(dir)?;
            let items = store
                .search("", Filter::Snippets, 10_000)
                .map_err(|e| e.to_string())?;
            print_items(&items, json)
        }
        Command::Get(target) => {
            let store = open(dir)?;
            let clip = resolve(&store, &target)?;
            let text = expanded_text(&store, &clip);
            let mut out = std::io::stdout().lock();
            out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            if !text.ends_with('\n') {
                let _ = writeln!(out);
            }
            Ok(())
        }
        Command::Copy { target, plain } => {
            let store = open(dir)?;
            let clip = resolve(&store, &target)?;
            copy(&store, &clip, plain)?;
            eprintln!(
                "{}",
                trf!(
                    "已复制：{}",
                    "Copied: {}",
                    clip.note.clone().unwrap_or(clip.title())
                )
            );
            Ok(())
        }
        Command::Add {
            text,
            snippet,
            note,
        } => {
            let text = match text.as_deref() {
                None | Some("-") => {
                    let mut buf = String::new();
                    std::io::stdin()
                        .lock()
                        .read_to_string(&mut buf)
                        .map_err(|e| e.to_string())?;
                    buf
                }
                Some(t) => t.to_string(),
            };
            let mut store = Store::open(dir).map_err(|e| e.to_string())?;
            let seq = store
                .record(NewClip {
                    payload: Payload::Text(text),
                    source: Some(tr("命令行", "Command line").into()),
                    from_self: false,
                    rich: Default::default(),
                })
                .map_err(|e| e.to_string())?
                .ok_or_else(|| tr("内容是空的", "Nothing to add").to_string())?;
            if snippet || note.is_some() {
                // 刚写入的就是最新的一条
                let clip = store
                    .search("", Filter::All, 1)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|c| c.seq == seq)
                    .ok_or_else(|| tr("写入失败", "Could not add it").to_string())?;
                if snippet {
                    store
                        .set_snippet(&clip.uuid, true)
                        .map_err(|e| e.to_string())?;
                }
                if let Some(note) = note {
                    store
                        .set_note(&clip.uuid, &note)
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        Command::Export { file, kept } => {
            let store = open(dir)?;
            let scope = if kept {
                ExportScope::Kept
            } else {
                ExportScope::All
            };
            let f = std::fs::File::create(&file)
                .map_err(|e| trf!("无法创建文件：{e}", "Could not create the file: {e}"))?;
            let n = store.export(
                scope,
                env!("CARGO_PKG_VERSION"),
                &mut std::io::BufWriter::new(f),
            )?;
            eprintln!(
                "{}",
                trf!(
                    "已导出 {n} 条到 {}",
                    "Exported {n} items to {}",
                    file.display()
                )
            );
            Ok(())
        }
        Command::Import { file } => {
            let f = std::fs::File::open(&file)
                .map_err(|e| trf!("无法打开文件：{e}", "Could not open the file: {e}"))?;
            let mut store = Store::open(dir).map_err(|e| e.to_string())?;
            let r = store.import(std::io::BufReader::new(f))?;
            let (added, merged, skipped) = (r.added, r.merged, r.skipped);
            eprintln!(
                "{}",
                trf!(
                    "导入完成：新增 {added} 条，合并 {merged} 条，跳过 {skipped} 行",
                    "Imported: {added} new, {merged} merged, {skipped} lines skipped"
                )
            );
            Ok(())
        }
    }
}

/// 序号（1 = 最新）、uuid 或片段名 → 记录（完整内容）
fn resolve(store: &Store, target: &Target) -> Result<Clip, String> {
    let not_found = || -> String { tr("没有找到这条记录", "No such item").into() };
    let listed = if let Some(name) = &target.snippet {
        let name = name.to_lowercase();
        store
            .search("", Filter::Snippets, 10_000)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|c| {
                c.note
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&name))
            })
            .ok_or_else(|| {
                trf!(
                    "没有备注里带「{name}」的片段（用 F2 给片段写备注）",
                    "No snippet whose note contains \"{name}\" (add notes with F2)"
                )
            })?
    } else {
        let item = target.item.as_deref().unwrap_or_default();
        match item.parse::<usize>() {
            Ok(n) if n >= 1 => store
                .search("", Filter::All, n)
                .map_err(|e| e.to_string())?
                .into_iter()
                .nth(n - 1)
                .ok_or_else(not_found)?,
            _ => store
                .get(item)
                .map_err(|e| e.to_string())?
                .ok_or_else(not_found)?,
        }
    };
    store
        .get(&listed.uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(not_found)
}

/// 片段展开变量后的文本；其他记录原样
fn expanded_text(store: &Store, clip: &Clip) -> String {
    if !clip.snippet || !template::has_variables(&clip.content) {
        return clip.content.clone();
    }
    let clipboard = store
        .search("", Filter::All, 5)
        .ok()
        .into_iter()
        .flatten()
        .find(|c| c.uuid != clip.uuid && !matches!(c.kind, Kind::Image | Kind::File))
        .and_then(|c| store.get(&c.uuid).ok().flatten())
        .map(|c| c.content)
        .unwrap_or_default();
    template::expand(
        &clip.content,
        &template::Context {
            now: local_time(),
            clipboard: &clipboard,
        },
    )
    .text
}

fn copy(store: &Store, clip: &Clip, plain: bool) -> Result<(), String> {
    let crlf = |s: &str| s.replace("\r\n", "\n").replace('\n', "\r\n");
    match clip.kind {
        Kind::File => {
            tessel_clipboard::write_standalone(WriteContent::Files(clip.files().collect()))
        }
        Kind::Image => {
            let path = &clip
                .image
                .as_ref()
                .ok_or(tr("图片文件丢失", "Image file is missing"))?
                .path;
            let png = std::fs::read(path).map_err(|e| e.to_string())?;
            tessel_clipboard::write_standalone(WriteContent::ImagePng(&png))
        }
        _ if clip.snippet && template::has_variables(&clip.content) => {
            let text = crlf(&expanded_text(store, clip));
            tessel_clipboard::write_standalone(WriteContent::Text(&text))
        }
        _ if clip.has_rich && !plain => {
            let rich = store.rich_formats(&clip.uuid).map_err(|e| e.to_string())?;
            tessel_clipboard::write_standalone(WriteContent::RichText {
                text: &clip.content,
                html: rich.html.as_deref(),
                rtf: rich.rtf.as_deref(),
            })
        }
        _ => tessel_clipboard::write_standalone(WriteContent::Text(&clip.content)),
    }
}

fn print_items(items: &[Clip], json: bool) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    if json {
        let rows: Vec<Item> = items
            .iter()
            .enumerate()
            .map(|(i, c)| Item {
                position: i + 1,
                uuid: &c.uuid,
                kind: c.kind.as_str(),
                title: c.title(),
                note: c.note.as_deref(),
                source: c.source.as_deref(),
                pinned: c.pinned,
                snippet: c.snippet,
                last_used_at: c.last_used_at,
            })
            .collect();
        let text = serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?;
        return writeln!(out, "{text}").map_err(|e| e.to_string());
    }
    let now = now_ms();
    for (i, c) in items.iter().enumerate() {
        let title: String = c
            .note
            .clone()
            .unwrap_or_else(|| c.title())
            .chars()
            .take(70)
            .collect();
        let mark = match (c.pinned, c.snippet) {
            (_, true) => "✂ ",
            (true, false) => "★ ",
            _ => "",
        };
        let _ = writeln!(
            out,
            "{:>3}  {mark}{title}\n     {} · {} · {}",
            i + 1,
            c.kind.label(),
            c.source.as_deref().unwrap_or(tr("未知来源", "Unknown app")),
            relative(c.last_used_at, now)
        );
    }
    if items.is_empty() {
        eprintln!("{}", tr("没有找到匹配的内容", "No matches"));
    }
    Ok(())
}
