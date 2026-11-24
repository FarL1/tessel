//! 片段里的模板变量：粘贴片段时把 `{date}` 之类替换成实际的值。
//!
//! 只对标记为"片段"的记录展开，普通历史里的 `{date}` 原样保留（比如复制的代码）。
//! 变量名中英文都可以写；不认识的 `{...}` 原样保留，所以不需要转义。
//!
//! | 变量 | 结果 |
//! |---|---|
//! | `{date}` / `{日期}` | 2026-11-24 |
//! | `{time}` / `{时间}` | 14:05 |
//! | `{datetime}` / `{日期时间}` | 2026-11-24 14:05 |
//! | `{weekday}` / `{星期}` | 星期二 / Tuesday（跟随界面语言） |
//! | `{clipboard}` / `{剪贴板}` | 上一次复制的文本 |
//! | `{cursor}` / `{光标}` | 粘贴后光标停在这里（只认第一个） |

use crate::i18n::is_en;

/// 本地时间（由调用方从系统读取，这里不依赖平台 API）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    /// 0 = 星期日
    pub weekday: u8,
}

pub struct Context<'a> {
    pub now: LocalTime,
    /// 上一次复制的文本，`{clipboard}` 用
    pub clipboard: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Expanded {
    /// 展开后的文本（换行是 `\n`）
    pub text: String,
    /// 粘贴后要按几次 ← 才能回到 `{cursor}` 的位置（一个换行按一次）
    pub cursor_back: usize,
}

const CURSOR: [&str; 2] = ["{cursor}", "{光标}"];

fn value(name: &str, ctx: &Context) -> Option<String> {
    let t = ctx.now;
    let date = format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
    let time = format!("{:02}:{:02}", t.hour, t.minute);
    Some(match name {
        "date" | "日期" => date,
        "time" | "时间" => time,
        "datetime" | "日期时间" => format!("{date} {time}"),
        "weekday" | "星期" => {
            const ZH: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
            const EN: [&str; 7] = [
                "Sunday",
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
            ];
            let i = usize::from(t.weekday % 7);
            if is_en() {
                EN[i].to_string()
            } else {
                format!("星期{}", ZH[i])
            }
        }
        "clipboard" | "剪贴板" => ctx.clipboard.replace("\r\n", "\n"),
        _ => return None,
    })
}

/// 文本里有没有可以替换的变量
pub fn has_variables(text: &str) -> bool {
    variables(text).next().is_some()
}

/// 文本里出现的变量（含花括号），按出现顺序；界面里高亮用
pub fn variables(text: &str) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut pos = 0;
    std::iter::from_fn(move || {
        while let Some(open) = text[pos..].find('{') {
            let start = pos + open;
            let Some(close) = text[start..].find('}') else {
                pos = text.len();
                return None;
            };
            let end = start + close + 1;
            let name = &text[start + 1..end - 1];
            pos = start + 1;
            if is_known(name) {
                pos = end;
                return Some(start..end);
            }
        }
        None
    })
}

fn is_known(name: &str) -> bool {
    let probe = Context {
        now: LocalTime::default(),
        clipboard: "",
    };
    value(name, &probe).is_some() || CURSOR.contains(&format!("{{{name}}}").as_str())
}

/// 展开变量
pub fn expand(text: &str, ctx: &Context) -> Expanded {
    let text = text.replace("\r\n", "\n");
    let mut out = String::with_capacity(text.len());
    let mut cursor_at: Option<usize> = None;
    let mut last = 0;
    for range in variables(&text).collect::<Vec<_>>() {
        out.push_str(&text[last..range.start]);
        let token = &text[range.clone()];
        if CURSOR.contains(&token) {
            // 只认第一个光标位置，其余的去掉
            cursor_at.get_or_insert(out.len());
        } else if let Some(v) = value(&token[1..token.len() - 1], ctx) {
            out.push_str(&v);
        }
        last = range.end;
    }
    out.push_str(&text[last..]);
    let cursor_back = cursor_at.map_or(0, |at| out[at..].chars().count());
    Expanded {
        text: out,
        cursor_back,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context<'static> {
        Context {
            now: LocalTime {
                year: 2026,
                month: 11,
                day: 4,
                hour: 9,
                minute: 5,
                weekday: 3,
            },
            clipboard: "订单号 A-1024\r\n",
        }
    }

    #[test]
    fn expands_known_variables() {
        let e = expand("日期 {date} {时间}，{weekday}。{datetime}", &ctx());
        assert_eq!(e.text, "日期 2026-11-04 09:05，星期三。2026-11-04 09:05");
        assert_eq!(e.cursor_back, 0);
        // 剪贴板内容统一成 \n 换行
        assert_eq!(
            expand("单号：{剪贴板}", &ctx()).text,
            "单号：订单号 A-1024\n"
        );
    }

    #[test]
    fn unknown_braces_are_kept() {
        let src = "fn main() { println!(\"{}\", x); } {name} {date";
        assert_eq!(expand(src, &ctx()).text, src);
        assert!(!has_variables(src));
        assert!(has_variables("x {cursor}"));
    }

    #[test]
    fn cursor_position() {
        let e = expand("您好，{cursor}\r\n祝好\r\n{date}", &ctx());
        assert_eq!(e.text, "您好，\n祝好\n2026-11-04");
        // 光标后面还有："\n祝好\n2026-11-04" = 1 + 2 + 1 + 10
        assert_eq!(e.cursor_back, 14);
        // 第二个光标标记去掉
        assert_eq!(expand("a{光标}b{cursor}c", &ctx()).text, "abc");
        assert_eq!(expand("a{光标}b{cursor}c", &ctx()).cursor_back, 2);
    }

    #[test]
    fn variable_ranges() {
        let text = "{date} 和 {x} 和 {cursor}";
        let found: Vec<&str> = variables(text).map(|r| &text[r]).collect();
        assert_eq!(found, vec!["{date}", "{cursor}"]);
    }
}
