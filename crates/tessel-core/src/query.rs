//! 搜索语法：普通关键词之外，支持几个前缀过滤，可以和关键词任意组合。
//!
//! | 写法 | 含义 |
//! |---|---|
//! | `app:微信` / `来源:微信` | 来源程序名包含"微信" |
//! | `type:link` / `类型:链接` | 只看某种类型：text code link color image file（或中文名） |
//! | `is:pinned` / `is:收藏` | 只看收藏 |
//! | `is:rich` / `is:格式` | 只看带格式的文本 |
//! | `#工作` / `tag:工作` / `标签:工作` | 备注里带 `#工作` 标签的 |
//! | `-词` | 排除包含这个词的记录 |
//!
//! `#` 后面是颜色值（`#4F6BED`）时仍按普通关键词搜索。
//! 不认识的前缀（比如 `http://` 里的冒号、`12:30`）按普通关键词处理。

use crate::model::Kind;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// 普通关键词（全文搜索、结果高亮用的就是这些）
    pub terms: Vec<String>,
    /// 排除的关键词
    pub excluded: Vec<String>,
    /// 来源程序名包含其中每一个
    pub sources: Vec<String>,
    /// 备注里的标签（不含 `#`）
    pub tags: Vec<String>,
    pub kind: Option<Kind>,
    pub pinned: bool,
    pub rich: bool,
}

impl Query {
    pub fn parse(input: &str) -> Self {
        let mut q = Query::default();
        for token in input.split_whitespace() {
            if let Some(word) = token.strip_prefix('-')
                && !word.is_empty()
            {
                q.excluded.push(word.to_string());
                continue;
            }
            if let Some(tag) = token.strip_prefix(['#', '＃'])
                && !tag.is_empty()
                && !is_hex_color(tag)
            {
                q.tags.push(tag.to_string());
                continue;
            }
            if let Some((key, value)) = token.split_once([':', '：'])
                && !value.is_empty()
                && q.apply_prefix(key, value)
            {
                continue;
            }
            q.terms.push(token.to_string());
        }
        q
    }

    /// 能识别就应用并返回 true
    fn apply_prefix(&mut self, key: &str, value: &str) -> bool {
        match key.to_lowercase().as_str() {
            "app" | "from" | "来源" => {
                self.sources.push(value.to_string());
                true
            }
            "tag" | "标签" => {
                self.tags
                    .push(value.trim_start_matches(['#', '＃']).to_string());
                true
            }
            "type" | "kind" | "类型" => match parse_kind(value) {
                Some(kind) => {
                    self.kind = Some(kind);
                    true
                }
                None => false,
            },
            "is" | "是" => match value.to_lowercase().as_str() {
                "pinned" | "pin" | "starred" | "收藏" => {
                    self.pinned = true;
                    true
                }
                "rich" | "formatted" | "格式" | "富文本" => {
                    self.rich = true;
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// 结果高亮用的关键词（空格分隔）
    pub fn highlight(&self) -> String {
        self.terms.join(" ")
    }
}

/// `#` 后面是不是颜色值（3 / 4 / 6 / 8 位十六进制）
fn is_hex_color(s: &str) -> bool {
    matches!(s.len(), 3 | 4 | 6 | 8) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 备注里的标签：以 `#` 开头的词（不含 `#`）。颜色值不算标签。
pub fn note_tags(note: &str) -> Vec<&str> {
    note.split_whitespace()
        .filter_map(|w| w.strip_prefix(['#', '＃']))
        .filter(|t| !t.is_empty() && !is_hex_color(t))
        .collect()
}

fn parse_kind(value: &str) -> Option<Kind> {
    let v = value.to_lowercase();
    Kind::ALL
        .into_iter()
        .find(|k| k.as_str() == v || k.label() == v)
        .or(match v.as_str() {
            "url" | "links" | "网址" => Some(Kind::Link),
            "img" | "picture" | "截图" => Some(Kind::Image),
            "files" | "path" => Some(Kind::File),
            "colour" => Some(Kind::Color),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_terms() {
        let q = Query::parse("会议  纪要");
        assert_eq!(q.terms, vec!["会议", "纪要"]);
        assert_eq!(q.highlight(), "会议 纪要");
    }

    #[test]
    fn prefixes() {
        let q = Query::parse("app:微信 type:链接 is:pinned 报销 -草稿");
        assert_eq!(q.sources, vec!["微信"]);
        assert_eq!(q.kind, Some(Kind::Link));
        assert!(q.pinned);
        assert_eq!(q.terms, vec!["报销"]);
        assert_eq!(q.excluded, vec!["草稿"]);

        // 全角冒号、中文键、大小写
        let q = Query::parse("来源：Chrome 类型:code IS:Rich");
        assert_eq!(q.sources, vec!["Chrome"]);
        assert_eq!(q.kind, Some(Kind::Code));
        assert!(q.rich);
        assert!(q.terms.is_empty());
    }

    #[test]
    fn tags() {
        let q = Query::parse("#工作 tag:报销 标签：#旅行 #4F6BED #fff");
        assert_eq!(q.tags, vec!["工作", "报销", "旅行"]);
        assert_eq!(q.terms, vec!["#4F6BED", "#fff"]);
        assert_eq!(note_tags("出差用 #工作 #旅行 #abc"), vec!["工作", "旅行"]);
    }

    #[test]
    fn unknown_prefixes_are_terms() {
        let q = Query::parse("https://a.com 12:30 type:unknown app: -");
        assert_eq!(
            q.terms,
            vec!["https://a.com", "12:30", "type:unknown", "app:", "-"]
        );
        assert!(q.sources.is_empty() && q.kind.is_none());
    }
}
