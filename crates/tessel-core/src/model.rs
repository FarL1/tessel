use std::path::PathBuf;

use crate::i18n::tr;
use crate::trf;

/// 内容类型。存储时用 `as_str()` 的值，改名时注意兼容旧数据。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Text,
    Link,
    Code,
    Color,
    Image,
    File,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Text,
        Kind::Link,
        Kind::Code,
        Kind::Color,
        Kind::Image,
        Kind::File,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Link => "link",
            Kind::Code => "code",
            Kind::Color => "color",
            Kind::Image => "image",
            Kind::File => "file",
        }
    }

    pub fn parse(s: &str) -> Kind {
        match s {
            "link" => Kind::Link,
            "code" => Kind::Code,
            "color" => Kind::Color,
            "image" => Kind::Image,
            "file" => Kind::File,
            _ => Kind::Text,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Text => tr("文本", "Text"),
            Kind::Link => tr("链接", "Link"),
            Kind::Code => tr("代码", "Code"),
            Kind::Color => tr("颜色", "Color"),
            Kind::Image => tr("图片", "Image"),
            Kind::File => tr("文件", "File"),
        }
    }

    /// 分类标签上的名字（英文用复数）
    pub fn category_label(self) -> &'static str {
        match self {
            Kind::Text => tr("文本", "Text"),
            Kind::Link => tr("链接", "Links"),
            Kind::Code => tr("代码", "Code"),
            Kind::Color => tr("颜色", "Colors"),
            Kind::Image => tr("图片", "Images"),
            Kind::File => tr("文件", "Files"),
        }
    }
}

/// 列表上方的分类筛选
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Kind(Kind),
    Pinned,
}

impl Filter {
    pub const ALL: [Filter; 8] = [
        Filter::All,
        Filter::Kind(Kind::Text),
        Filter::Kind(Kind::Link),
        Filter::Kind(Kind::Code),
        Filter::Kind(Kind::Image),
        Filter::Kind(Kind::File),
        Filter::Kind(Kind::Color),
        Filter::Pinned,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => tr("全部", "All"),
            Filter::Kind(k) => k.category_label(),
            Filter::Pinned => tr("收藏", "Pinned"),
        }
    }
}

/// 从剪贴板采集到的原始内容
#[derive(Clone, Debug)]
pub enum Payload {
    Text(String),
    Files(Vec<String>),
    /// PNG 编码的图片
    Image {
        png: Vec<u8>,
        width: u32,
        height: u32,
    },
}

/// 文本附带的富格式（网页、Word、Excel、邮件等复制时会带上）。
/// 按剪贴板里的原始字节保存，粘贴时原样写回，保证格式不走样。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichFormats {
    /// Windows `HTML Format`（CF_HTML：带偏移量头的 UTF-8）
    pub html: Option<Vec<u8>>,
    /// `Rich Text Format`
    pub rtf: Option<Vec<u8>>,
}

impl RichFormats {
    pub fn is_empty(&self) -> bool {
        self.html.is_none() && self.rtf.is_none()
    }
}

/// 准备写入存储的一条记录
#[derive(Clone, Debug)]
pub struct NewClip {
    pub payload: Payload,
    /// 只对文本有意义
    pub rich: RichFormats,
    /// 来源应用的显示名（如 "Google Chrome"）
    pub source: Option<String>,
    /// 是否是 Tessel 自己写入剪贴板的（粘贴时）。这种情况只把已有记录提到最前，不改来源。
    pub from_self: bool,
}

/// 存储中的一条记录
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    /// 最近使用顺序：数字越大越新（再次使用时会重新分配）
    pub seq: i64,
    /// 跨设备稳定的 ID（UUIDv7），同步时使用
    pub uuid: String,
    pub kind: Kind,
    /// 文本形式的内容：文本原文；文件为换行分隔的路径；图片为描述
    pub content: String,
    pub source: Option<String>,
    /// Unix 毫秒
    pub created_at: i64,
    pub last_used_at: i64,
    pub use_count: i64,
    pub pinned: bool,
    /// 代码的语言（识别结果），例如 "Rust"
    pub lang: Option<String>,
    pub image: Option<ImageInfo>,
    /// 是否保存了富格式（HTML / RTF）；内容本身用 `Store::rich_formats()` 读取
    pub has_rich: bool,
    /// 用户写的备注，可以带 `#标签`
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageInfo {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

impl Clip {
    /// 列表里显示的标题：第一行非空文本；链接去掉协议头；文件显示文件名
    pub fn title(&self) -> String {
        match self.kind {
            Kind::Link => self
                .content
                .trim()
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_string(),
            Kind::File => {
                let files: Vec<&str> = self.files().collect();
                let first = files
                    .first()
                    .map(|f| file_name(f))
                    .unwrap_or_default()
                    .to_string();
                if files.len() > 1 {
                    let n = files.len();
                    let more = n - 1;
                    trf!("{first} 等 {n} 个文件", "{first} and {more} more")
                } else {
                    first
                }
            }
            Kind::Image => match &self.image {
                Some(img) => trf!("图片 {} × {}", "Image {} × {}", img.width, img.height),
                None => tr("图片", "Image").to_string(),
            },
            _ => self
                .content
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect(),
        }
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.content.lines().filter(|l| !l.trim().is_empty())
    }

    pub fn char_count(&self) -> usize {
        self.content.chars().count()
    }

    pub fn line_count(&self) -> usize {
        self.content.lines().count().max(1)
    }

    pub fn domain(&self) -> &str {
        let rest = self
            .content
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        rest.split(['/', '?', '#']).next().unwrap_or(rest)
    }
}

pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}
