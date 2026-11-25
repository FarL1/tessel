//! 对一条记录的内容做转换：格式化 JSON、去掉链接追踪参数、转换颜色格式……
//!
//! 动作菜单（Ctrl+K）只列出对当前记录适用、并且结果和原文不同的转换（见 [`available`]）。
//! 输出统一用 `\n` 换行，写回剪贴板时由平台层转换。

use crate::classify::parse_hex_rgba;
use crate::i18n::tr;
use crate::model::{Clip, Kind};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Transform {
    FormatJson,
    MinifyJson,
    StripTracking,
    UrlDecode,
    ColorHex,
    ColorRgb,
    ColorHsl,
    TidyWhitespace,
    JoinLines,
    Uppercase,
    Lowercase,
}

impl Transform {
    pub const ALL: [Transform; 11] = [
        Transform::FormatJson,
        Transform::MinifyJson,
        Transform::StripTracking,
        Transform::UrlDecode,
        Transform::ColorHex,
        Transform::ColorRgb,
        Transform::ColorHsl,
        Transform::TidyWhitespace,
        Transform::JoinLines,
        Transform::Uppercase,
        Transform::Lowercase,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Transform::FormatJson => tr("格式化 JSON", "Format JSON"),
            Transform::MinifyJson => tr("压缩 JSON", "Minify JSON"),
            Transform::StripTracking => tr("去掉链接追踪参数", "Remove tracking parameters"),
            Transform::UrlDecode => tr("URL 解码", "URL-decode"),
            Transform::ColorHex => tr("转为 HEX", "Convert to HEX"),
            Transform::ColorRgb => tr("转为 RGB", "Convert to RGB"),
            Transform::ColorHsl => tr("转为 HSL", "Convert to HSL"),
            Transform::TidyWhitespace => tr("清理空白", "Tidy whitespace"),
            Transform::JoinLines => tr("合并为一行", "Join into one line"),
            Transform::Uppercase => tr("转为大写", "UPPERCASE"),
            Transform::Lowercase => tr("转为小写", "lowercase"),
        }
    }

    /// 在动作菜单里搜索时额外匹配的关键词
    pub fn keywords(self) -> &'static str {
        match self {
            Transform::FormatJson => "json format pretty 美化 格式化",
            Transform::MinifyJson => "json minify compact 一行",
            Transform::StripTracking => "url link utm clean tracking remove 清理 跟踪 追踪",
            Transform::UrlDecode => "url decode percent 中文",
            Transform::ColorHex => "color hex 颜色",
            Transform::ColorRgb => "color rgb 颜色",
            Transform::ColorHsl => "color hsl 颜色",
            Transform::TidyWhitespace => "trim whitespace tidy 空格 空行",
            Transform::JoinLines => "join lines one line pdf 换行 合并",
            Transform::Uppercase => "upper case uppercase 大小写 大写",
            Transform::Lowercase => "lower case lowercase 大小写 小写",
        }
    }

    /// 转换结果；不适用时返回 `None`
    pub fn apply(self, clip: &Clip) -> Option<String> {
        if matches!(clip.kind, Kind::Image | Kind::File) {
            return None;
        }
        let text = clip.content.as_str();
        match self {
            Transform::FormatJson => json(text, true),
            Transform::MinifyJson => json(text, false),
            Transform::StripTracking => (clip.kind == Kind::Link)
                .then(|| strip_tracking(text))
                .flatten(),
            Transform::UrlDecode => url_decode(text.trim()),
            Transform::ColorHex => parse_color(text).map(|c| c.hex()),
            Transform::ColorRgb => parse_color(text).map(|c| c.rgb()),
            Transform::ColorHsl => parse_color(text).map(|c| c.hsl()),
            Transform::TidyWhitespace => {
                matches!(clip.kind, Kind::Text | Kind::Code).then(|| tidy_whitespace(text))
            }
            Transform::JoinLines => (clip.kind == Kind::Text).then(|| join_lines(text)),
            Transform::Uppercase => {
                (clip.kind == Kind::Text && has_ascii_alpha(text)).then(|| text.to_uppercase())
            }
            Transform::Lowercase => {
                (clip.kind == Kind::Text && has_ascii_alpha(text)).then(|| text.to_lowercase())
            }
        }
    }
}

/// 对这条记录可用的转换和对应结果（结果为空或和原文相同的不列出）
pub fn available(clip: &Clip) -> Vec<(Transform, String)> {
    let same = |a: &str, b: &str| {
        let norm = |s: &str| -> String {
            s.chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect()
        };
        // 颜色只是写法不同（大小写、空格）不算变化
        if clip.kind == Kind::Color {
            norm(a) == norm(b)
        } else {
            a.replace("\r\n", "\n") == b.replace("\r\n", "\n")
        }
    };
    Transform::ALL
        .iter()
        .filter_map(|&t| {
            let out = t.apply(clip)?;
            (!out.trim().is_empty() && !same(&out, &clip.content)).then_some((t, out))
        })
        .collect()
}

// ───────────────────────────── JSON ─────────────────────────────

/// 先校验是合法 JSON，再按词法重新排版。
/// 不经过解析再序列化，所以数字、字符串、键的顺序都原样保留（大整数不会丢精度）。
fn json(text: &str, pretty: bool) -> Option<String> {
    let t = text.trim();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return None;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(t).ok()?;
    Some(reformat_json(t, pretty))
}

fn reformat_json(src: &str, pretty: bool) -> String {
    fn newline(out: &mut String, indent: usize) {
        out.push('\n');
        for _ in 0..indent {
            out.push_str("  ");
        }
    }
    let mut out = String::with_capacity(src.len() + src.len() / 4);
    let mut indent = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            ' ' | '\t' | '\n' | '\r' => {}
            '{' | '[' => {
                out.push(c);
                while chars.next_if(|c| c.is_ascii_whitespace()).is_some() {}
                // 空对象、空数组保持 {} []
                if let Some(close) = chars.next_if(|&c| c == '}' || c == ']') {
                    out.push(close);
                    continue;
                }
                indent += 1;
                if pretty {
                    newline(&mut out, indent);
                }
            }
            '}' | ']' => {
                indent = indent.saturating_sub(1);
                if pretty {
                    newline(&mut out, indent);
                }
                out.push(c);
            }
            ',' => {
                out.push(c);
                if pretty {
                    newline(&mut out, indent);
                }
            }
            ':' => {
                out.push(':');
                if pretty {
                    out.push(' ');
                }
            }
            _ => out.push(c),
        }
    }
    out
}

// ───────────────────────────── 链接 ─────────────────────────────

/// 各站点通用的追踪参数（另外所有 `utm_` 开头的参数也会去掉）
const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "twclid",
    "ttclid",
    "li_fat_id",
    "mc_cid",
    "mc_eid",
    "igshid",
    "igsh",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "vero_id",
    "oly_anon_id",
    "oly_enc_id",
    "rb_clickid",
    "ref_src",
    "ref_url",
    "spm",
    "spm_id_from",
    "scm",
    "vd_source",
    "share_source",
    "share_medium",
    "share_plat",
    "share_session_id",
    "share_tag",
    "share_from",
    "unique_k",
];

/// 只在特定网站上才是追踪参数（在别处可能有实际含义）
const SITE_TRACKING_PARAMS: &[(&[&str], &[&str])] = &[
    (
        &["youtube.com", "youtu.be", "spotify.com"],
        &["si", "feature"],
    ),
    (&["x.com", "twitter.com"], &["s", "t"]),
    (
        &["bilibili.com", "b23.tv"],
        &[
            "buvid",
            "from_spmid",
            "is_story_h5",
            "mid",
            "plat_id",
            "timestamp",
            "up_id",
            "bbid",
            "ts",
        ],
    ),
    (
        &["xiaohongshu.com"],
        &["xhsshare", "appuid", "apptime", "share_id", "author_share"],
    ),
    (&["zhihu.com"], &["share_code"]),
    (
        &["mp.weixin.qq.com"],
        &[
            "chksm",
            "scene",
            "subscene",
            "sharer_shareinfo",
            "sharer_shareinfo_first",
            "sharer_sharetime",
            "sharer_shareid",
            "clicktime",
            "enterid",
            "sessionid",
            "isappinstalled",
            "from",
            "exportkey",
            "pass_ticket",
            "wx_header",
            "ascene",
            "devicetype",
            "version",
            "nettype",
            "abtest_cookie",
            "lang",
            "countrycode",
            "acctmode",
            "key",
            "srcid",
            "mpshare",
            "realreporttime",
        ],
    ),
    (
        &["taobao.com", "tmall.com"],
        &[
            "ut_sk",
            "sourcetype",
            "suid",
            "shareurl",
            "un",
            "share_crt_v",
            "sp_tk",
            "cpp",
            "short_name",
            "app",
        ],
    ),
];

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

fn is_tracking(key: &str, host: &str) -> bool {
    let key = key.to_ascii_lowercase();
    if key.starts_with("utm_") || TRACKING_PARAMS.contains(&key.as_str()) {
        return true;
    }
    SITE_TRACKING_PARAMS.iter().any(|(sites, params)| {
        sites
            .iter()
            .any(|s| host == *s || host.ends_with(&format!(".{s}")))
            && params.contains(&key.as_str())
    })
}

fn strip_tracking(url: &str) -> Option<String> {
    let url = url.trim();
    let (base, fragment) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let (path, query) = base.split_once('?')?;
    let host = host_of(path);
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| {
            let key = p.split('=').next().unwrap_or(p);
            !p.is_empty() && !is_tracking(key, &host)
        })
        .collect();
    let mut out = path.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    Some(out)
}

/// 解码 `%XX`（不处理 `+`：它只在表单编码里表示空格，链接路径里是原样的加号）。
/// 解码后不是合法 UTF-8 时不适用。
fn url_decode(text: &str) -> Option<String> {
    if !text.contains('%') {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

// ───────────────────────────── 颜色 ─────────────────────────────

/// 0–255 的 RGB 和 0–1 的透明度
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Color {
    pub fn hex(self) -> String {
        let Color { r, g, b, a } = self;
        if a < 1.0 {
            format!("#{r:02X}{g:02X}{b:02X}{:02X}", (a * 255.0).round() as u8)
        } else {
            format!("#{r:02X}{g:02X}{b:02X}")
        }
    }

    pub fn rgb(self) -> String {
        let Color { r, g, b, a } = self;
        if a < 1.0 {
            format!("rgba({r}, {g}, {b}, {})", fmt_alpha(a))
        } else {
            format!("rgb({r}, {g}, {b})")
        }
    }

    pub fn hsl(self) -> String {
        let (h, s, l) = self.to_hsl();
        let (h, s, l) = (
            h.round() as u32 % 360,
            (s * 100.0).round(),
            (l * 100.0).round(),
        );
        if self.a < 1.0 {
            format!("hsla({h}, {s}%, {l}%, {})", fmt_alpha(self.a))
        } else {
            format!("hsl({h}, {s}%, {l}%)")
        }
    }

    /// (色相 0–360, 饱和度 0–1, 亮度 0–1)
    pub fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d == 0.0 {
            return (0.0, 0.0, l);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            60.0 * (((g - b) / d).rem_euclid(6.0))
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        (h, s, l)
    }
}

fn fmt_alpha(a: f32) -> String {
    let s = format!("{:.2}", a);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 解析 `#RGB` `#RRGGBB` `#RRGGBBAA`、`rgb()` `rgba()` `hsl()` `hsla()`（逗号或空格分隔都可以）
pub fn parse_color(text: &str) -> Option<Color> {
    let t = text.trim();
    if t.starts_with('#') {
        let v = parse_hex_rgba(t)?;
        return Some(Color {
            r: (v >> 24) as u8,
            g: (v >> 16) as u8,
            b: (v >> 8) as u8,
            a: (v & 0xFF) as f32 / 255.0,
        });
    }
    let lower = t.to_ascii_lowercase();
    let (func, rest) = lower.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    let parts: Vec<&str> = args
        .split([',', ' ', '/'])
        .filter(|s| !s.is_empty())
        .collect();
    if !(3..=4).contains(&parts.len()) {
        return None;
    }
    let alpha = match parts.get(3) {
        Some(a) => match a.strip_suffix('%') {
            Some(p) => p.parse::<f32>().ok()? / 100.0,
            None => a.parse::<f32>().ok()?,
        }
        .clamp(0.0, 1.0),
        None => 1.0,
    };
    let channel = |s: &str| -> Option<u8> {
        let v = match s.strip_suffix('%') {
            Some(p) => p.parse::<f32>().ok()? * 2.55,
            None => s.parse::<f32>().ok()?,
        };
        Some(v.clamp(0.0, 255.0).round() as u8)
    };
    let percent = |s: &str| -> Option<f32> {
        Some((s.strip_suffix('%').unwrap_or(s).parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0))
    };
    match func.trim() {
        "rgb" | "rgba" => Some(Color {
            r: channel(parts[0])?,
            g: channel(parts[1])?,
            b: channel(parts[2])?,
            a: alpha,
        }),
        "hsl" | "hsla" => {
            let h = parts[0].strip_suffix("deg").unwrap_or(parts[0]);
            let h = h.parse::<f32>().ok()?.rem_euclid(360.0);
            let (s, l) = (percent(parts[1])?, percent(parts[2])?);
            let (r, g, b) = hsl_to_rgb(h, s, l);
            Some(Color { r, g, b, a: alpha })
        }
        _ => None,
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to(r), to(g), to(b))
}

// ───────────────────────────── 文本 ─────────────────────────────

/// 去掉行尾空白和首尾空行，连续多个空行合并成一个，不间断空格换成普通空格。
/// 行首缩进保留（代码要用）。
fn tidy_whitespace(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.replace('\u{00A0}', " ").trim_end().to_string();
        if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{2E80}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}') || is_cjk_punct(c)
}

/// 中文标点、全角符号
fn is_cjk_punct(c: char) -> bool {
    matches!(c, '\u{3000}'..='\u{303F}' | '\u{FF00}'..='\u{FFEF}')
}

/// 把多行合并成一行（比如从 PDF 复制的段落）。两边都是中文、或挨着中文标点时直接相连，
/// 其他情况用空格隔开。
fn join_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let (Some(prev), Some(next)) = (out.chars().last(), line.chars().next())
            && !(is_cjk(prev) && is_cjk(next))
            && !is_cjk_punct(prev)
            && !is_cjk_punct(next)
        {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

fn has_ascii_alpha(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::classify_text;

    fn clip(content: &str) -> Clip {
        let (kind, lang) = classify_text(content);
        Clip {
            seq: 1,
            uuid: "u".into(),
            kind,
            content: content.into(),
            source: None,
            created_at: 0,
            last_used_at: 0,
            use_count: 1,
            pinned: false,
            lang,
            image: None,
            has_rich: false,
            note: None,
            snippet: false,
            ocr: None,
        }
    }

    fn labels(content: &str) -> Vec<Transform> {
        available(&clip(content))
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn json_keeps_order_and_numbers() {
        let src = r#"{"z":1,"a":[1,2,{}],"big":12345678901234567890123,"s":"a,b:{c}\"]"}"#;
        let pretty = json(src, true).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"z\": 1,\n  \"a\": [\n    1,\n    2,\n    {}\n  ],\n  \"big\": 12345678901234567890123,\n  \"s\": \"a,b:{c}\\\"]\"\n}"
        );
        assert_eq!(json(&pretty, false).unwrap(), src);
        assert_eq!(json("[ ]", true).unwrap(), "[]");
        assert!(json("{\"a\": }", true).is_none());
        assert!(json("hello", true).is_none());
    }

    #[test]
    fn json_transforms_listed_only_when_they_change_something() {
        let compact = r#"{"a":1,"b":[true,null]}"#;
        assert_eq!(labels(compact), vec![Transform::FormatJson]);
        let pretty = available(&clip(compact)).remove(0).1;
        assert_eq!(labels(&pretty), vec![Transform::MinifyJson]);
    }

    #[test]
    fn tracking_params() {
        let t = |u: &str| strip_tracking(u).unwrap();
        assert_eq!(
            t("https://example.com/a?id=3&utm_source=x&utm_medium=y#top"),
            "https://example.com/a?id=3#top"
        );
        assert_eq!(t("https://example.com/?fbclid=abc"), "https://example.com/");
        assert_eq!(
            t("https://www.bilibili.com/video/BV1?spm_id_from=333&vd_source=ab&t=30"),
            "https://www.bilibili.com/video/BV1?t=30"
        );
        assert_eq!(
            t("https://youtu.be/abc?si=xyz&t=5"),
            "https://youtu.be/abc?t=5"
        );
        // si 只在特定网站上去掉
        assert_eq!(t("https://example.com/?si=1"), "https://example.com/?si=1");
        assert_eq!(
            labels("https://example.com/a?id=3"),
            Vec::<Transform>::new()
        );
        assert!(labels("https://example.com/a?utm_source=x").contains(&Transform::StripTracking));
    }

    #[test]
    fn url_decoding() {
        assert_eq!(
            url_decode("https://zh.wikipedia.org/wiki/%E6%8B%BC%E8%B4%B4").unwrap(),
            "https://zh.wikipedia.org/wiki/拼贴"
        );
        assert_eq!(url_decode("100%").unwrap(), "100%");
        assert!(url_decode("%FF%FE").is_none());
        assert!(url_decode("plain").is_none());
    }

    #[test]
    fn colors() {
        let c = parse_color("#4F6BED").unwrap();
        assert_eq!(c.rgb(), "rgb(79, 107, 237)");
        assert_eq!(c.hsl(), "hsl(229, 81%, 62%)");
        assert_eq!(parse_color("rgb(79 107 237)").unwrap().hex(), "#4F6BED");
        assert_eq!(parse_color("hsl(229, 81%, 62%)").unwrap().hex(), "#506CED");
        assert_eq!(
            parse_color("rgba(0, 0, 0, 0.5)").unwrap().hex(),
            "#00000080"
        );
        assert_eq!(
            parse_color("#00000080").unwrap().rgb(),
            "rgba(0, 0, 0, 0.5)"
        );
        assert_eq!(
            labels("#4f6bed"),
            vec![Transform::ColorRgb, Transform::ColorHsl]
        );
    }

    #[test]
    fn text_transforms() {
        assert_eq!(
            tidy_whitespace("\n\n  a  \n\n\n\nb\u{00A0}\n\n"),
            "  a\n\nb"
        );
        assert_eq!(
            join_lines("这是一段从 PDF\n复制的文字，\nand some\nEnglish words"),
            "这是一段从 PDF 复制的文字，and some English words"
        );
        let l = labels("Hello World");
        assert!(l.contains(&Transform::Uppercase) && l.contains(&Transform::Lowercase));
        assert!(!labels("你好").contains(&Transform::Uppercase));
    }

    #[test]
    fn images_and_files_have_no_text_transforms() {
        let mut c = clip("C:\\a.txt");
        c.kind = Kind::File;
        assert!(available(&c).is_empty());
    }
}
