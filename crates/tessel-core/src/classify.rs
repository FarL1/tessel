//! 内容识别：把一段文本归类为 链接 / 颜色 / 代码 / 普通文本，并猜测代码语言。

use crate::model::Kind;

pub fn classify_text(text: &str) -> (Kind, Option<String>) {
    let t = text.trim();
    if is_link(t) {
        return (Kind::Link, None);
    }
    if is_color(t) {
        return (Kind::Color, None);
    }
    if let Some(lang) = code_language(text) {
        return (Kind::Code, Some(lang.to_string()));
    }
    (Kind::Text, None)
}

fn is_link(t: &str) -> bool {
    !t.contains(char::is_whitespace)
        && (t.starts_with("https://") || t.starts_with("http://") || t.starts_with("www."))
        && t.len() > 8
}

/// `#RGB` `#RRGGBB` `#RRGGBBAA`，以及 `rgb()` / `rgba()` / `hsl()` / `hsla()`
pub fn is_color(t: &str) -> bool {
    if let Some(hex) = t.strip_prefix('#') {
        return matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let lower = t.to_ascii_lowercase();
    ["rgb(", "rgba(", "hsl(", "hsla("]
        .iter()
        .any(|p| lower.starts_with(p))
        && lower.ends_with(')')
        && t.len() < 40
}

/// 解析 `#RGB` / `#RRGGBB` / `#RRGGBBAA` 为 0xRRGGBBAA
pub fn parse_hex_rgba(t: &str) -> Option<u32> {
    let hex = t.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        3 => {
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            Some(((r * 17) << 24) | ((g * 17) << 16) | ((b * 17) << 8) | 0xFF)
        }
        6 => Some((v << 8) | 0xFF),
        8 => Some(v),
        _ => None,
    }
}

/// 粗略判断是不是代码，并猜语言。宁可漏判（当普通文本），也不要把聊天记录误判成代码。
pub fn code_language(text: &str) -> Option<&'static str> {
    let t = text.trim();
    let lines: Vec<&str> = t.lines().collect();
    let has = |s: &str| t.contains(s);

    // JSON：以 { 或 [ 开头结尾，并且有 "key":
    if ((t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')))
        && has("\":")
    {
        return Some("JSON");
    }
    let upper = t.to_ascii_uppercase();
    let sql_start = [
        "SELECT ", "INSERT ", "UPDATE ", "DELETE ", "WITH ", "CREATE ",
    ]
    .iter()
    .any(|k| upper.starts_with(k));
    let sql_body = [" FROM ", "\nFROM ", " INTO ", " TABLE ", " SET "]
        .iter()
        .any(|k| upper.contains(k));
    if sql_start && sql_body {
        return Some("SQL");
    }
    if t.starts_with("<!") || (t.starts_with('<') && has("</") && has(">")) {
        return Some("HTML");
    }

    // 下面这些只看多行文本
    if lines.len() < 2 {
        return None;
    }
    if has("fn ") && (has("->") || has("let ") || has("::") || has("{")) {
        return Some("Rust");
    }
    if (has("def ") && has("):")) || (has("import ") && has("self.")) {
        return Some("Python");
    }
    if has("func ") && has("package ") {
        return Some("Go");
    }
    if has("#include") {
        return Some("C/C++");
    }
    if has("function ") || has("=> {") || (has("const ") && has(" = ") && has(";")) {
        return Some("JavaScript");
    }
    if (has("public class ") || has("private ")) && has(";") {
        return Some("Java");
    }

    // 通用信号：花括号、分号结尾，并且有缩进
    let symbolic = lines
        .iter()
        .filter(|l| {
            let l = l.trim_end();
            l.ends_with('{') || l.ends_with('}') || l.ends_with(';') || l.ends_with("):")
        })
        .count();
    let indented = lines
        .iter()
        .filter(|l| l.starts_with("    ") || l.starts_with('\t'))
        .count();
    if symbolic * 2 >= lines.len() && indented >= 1 {
        return Some("代码");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        assert_eq!(classify_text("https://github.com/a/b").0, Kind::Link);
        assert_eq!(classify_text("  www.example.com/x  ").0, Kind::Link);
        assert_eq!(classify_text("看看 https://a.com 这个").0, Kind::Text);
    }

    #[test]
    fn colors() {
        assert_eq!(classify_text("#4F6BED").0, Kind::Color);
        assert_eq!(classify_text("#fff").0, Kind::Color);
        assert_eq!(classify_text("rgb(1, 2, 3)").0, Kind::Color);
        assert_eq!(classify_text("#4F6BEZ").0, Kind::Text);
        assert_eq!(parse_hex_rgba("#fff"), Some(0xFFFFFFFF));
        assert_eq!(parse_hex_rgba("#4F6BED"), Some(0x4F6BEDFF));
    }

    #[test]
    fn code() {
        let rust = "fn main() {\n    println!(\"hi\");\n}";
        assert_eq!(classify_text(rust), (Kind::Code, Some("Rust".into())));
        assert_eq!(
            classify_text("SELECT id FROM clips WHERE seq > 1")
                .1
                .as_deref(),
            Some("SQL")
        );
        assert_eq!(classify_text("{\"a\": 1}").1.as_deref(), Some("JSON"));
        // 普通中文多行文本不应被当成代码
        let chat = "会议纪要：周三下午 3 点评审。\n1. 粘贴队列放进 M2\n2. 同步协议先做桌面";
        assert_eq!(classify_text(chat).0, Kind::Text);
    }
}
