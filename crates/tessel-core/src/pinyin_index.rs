//! 拼音搜索：输入拼音首字母（`hyjy`）或全拼（`huiyi`）也能找到中文内容。
//!
//! 存储时为含汉字的文本生成一个索引字符串 `"<首字母> <全拼>"`（只取前 2000 个字符，
//! 多音字取最常用的读音），和正文一起放进全文索引。界面高亮时用 [`match_ranges`]
//! 把拼音匹配映射回原文里的汉字。

use std::ops::Range;

use pinyin::ToPinyin;

/// 参与拼音索引的最大字符数
const MAX_CHARS: usize = 2000;

/// 汉字的拼音（不带声调，ü 写作 v，和输入法一致）
fn syllable(c: char) -> Option<String> {
    c.to_pinyin().map(|p| p.plain().replace('ü', "v"))
}

/// 生成拼音索引；不含汉字时返回 `None`
pub fn index_text(text: &str) -> Option<String> {
    let mut initials = String::new();
    let mut full = String::new();
    let mut has_han = false;
    for c in text.chars().take(MAX_CHARS) {
        if let Some(s) = syllable(c) {
            has_han = true;
            initials.push_str(&s[..1]);
            full.push_str(&s);
        } else if c.is_ascii_alphanumeric() {
            // 夹在中文里的英文、数字也保留，"Q3hy" 这样混着输入也能找到
            let c = c.to_ascii_lowercase();
            initials.push(c);
            full.push(c);
        }
    }
    has_han.then(|| format!("{initials} {full}"))
}

/// 这个搜索词是否可能是拼音（只有小写字母，或者首字母加数字）
pub fn is_pinyin_query(term: &str) -> bool {
    term.len() >= 2
        && term.bytes().all(|b| b.is_ascii_alphanumeric())
        && term.bytes().any(|b| b.is_ascii_alphabetic())
}

/// 在 `text` 里找出能用拼音匹配 `term` 的汉字片段（字节区间），用于高亮。
/// 每个汉字可以用全拼或首字母匹配，夹在中间的英文数字按原样匹配；至少包含一个汉字才算。
pub fn match_ranges(text: &str, term: &str) -> Vec<Range<usize>> {
    let term = term.to_ascii_lowercase();
    if !is_pinyin_query(&term) {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if syllable(chars[i].1).is_some()
            && let Some(end) = match_from(&chars, i, term.as_bytes(), false)
        {
            let start = chars[i].0;
            let end_byte = chars.get(end).map(|(b, _)| *b).unwrap_or(text.len());
            ranges.push(start..end_byte);
            i = end;
        } else {
            i += 1;
        }
    }
    ranges
}

/// 从第 `i` 个字符开始匹配 `term`，返回匹配结束的位置（不含）
fn match_from(chars: &[(usize, char)], i: usize, term: &[u8], seen_han: bool) -> Option<usize> {
    if term.is_empty() {
        return seen_han.then_some(i);
    }
    let &(_, c) = chars.get(i)?;
    if let Some(s) = syllable(c) {
        // 先试全拼，再试首字母
        if term.starts_with(s.as_bytes())
            && let Some(end) = match_from(chars, i + 1, &term[s.len()..], true)
        {
            return Some(end);
        }
        if term[0] == s.as_bytes()[0] {
            return match_from(chars, i + 1, &term[1..], true);
        }
        None
    } else if c.is_ascii_alphanumeric() && c.to_ascii_lowercase() as u8 == term[0] {
        match_from(chars, i + 1, &term[1..], seen_han)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index() {
        assert_eq!(index_text("会议纪要").as_deref(), Some("hyjy huiyijiyao"));
        assert_eq!(
            index_text("Q3 绿色报告！").as_deref(),
            Some("q3lsbg q3lvsebaogao")
        );
        assert_eq!(index_text("plain english 123"), None);
    }

    #[test]
    fn highlight() {
        let text = "明天的会议纪要";
        let hit = |term: &str| -> Vec<&str> {
            match_ranges(text, term)
                .into_iter()
                .map(|r| &text[r])
                .collect()
        };
        assert_eq!(hit("hyjy"), vec!["会议纪要"]);
        assert_eq!(hit("huiyi"), vec!["会议"]);
        // 全拼和首字母混着输入
        assert_eq!(hit("huiyjy"), vec!["会议纪要"]);
        assert_eq!(hit("HY"), vec!["会议"]);
        assert!(hit("xyz").is_empty());
        // 纯英文不算拼音匹配
        assert!(match_ranges("abc 会议", "ab").is_empty());
        assert!(!is_pinyin_query("会议"));
        assert!(!is_pinyin_query("a"));
    }
}
