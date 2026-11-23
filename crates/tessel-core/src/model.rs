use std::path::PathBuf;

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
            Kind::Text => "文本",
            Kind::Link => "链接",
            Kind::Code => "代码",
            Kind::Color => "颜色",
            Kind::Image => "图片",
            Kind::File => "文件",
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
            Filter::All => "全部",
            Filter::Kind(k) => k.label(),
            Filter::Pinned => "收藏",
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

/// 准备写入存储的一条记录
#[derive(Clone, Debug)]
pub struct NewClip {
    pub payload: Payload,
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
                    format!("{first} 等 {} 个文件", files.len())
                } else {
                    first
                }
            }
            Kind::Image => match &self.image {
                Some(img) => format!("图片 {} × {}", img.width, img.height),
                None => "图片".to_string(),
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
