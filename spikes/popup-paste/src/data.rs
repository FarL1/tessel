//! 原型用的模拟数据。正式版本里这些数据来自 mypaste-store。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Link,
    Code,
    Color,
    Image,
    File,
}

impl Kind {
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

/// 顶部的分类筛选
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Kind(Kind),
    Pinned,
}

pub const FILTERS: &[Filter] = &[
    Filter::All,
    Filter::Kind(Kind::Text),
    Filter::Kind(Kind::Link),
    Filter::Kind(Kind::Code),
    Filter::Kind(Kind::Image),
    Filter::Kind(Kind::File),
    Filter::Kind(Kind::Color),
    Filter::Pinned,
];

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "全部",
            Filter::Kind(k) => k.label(),
            Filter::Pinned => "收藏",
        }
    }

    pub fn matches(self, clip: &Clip) -> bool {
        match self {
            Filter::All => true,
            Filter::Kind(k) => clip.kind == k,
            Filter::Pinned => clip.pinned,
        }
    }
}

pub struct Clip {
    pub kind: Kind,
    /// 粘贴时写入剪贴板的文本
    pub content: &'static str,
    /// 图片等非文本内容的说明（尺寸、格式、大小）
    pub meta: &'static str,
    pub source: &'static str,
    pub ago: &'static str,
    pub pinned: bool,
}

impl Clip {
    /// 列表里显示的标题：第一行，链接去掉协议头，文件显示文件名
    pub fn title(&self) -> String {
        match self.kind {
            Kind::Link => self
                .content
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_string(),
            Kind::File => {
                let files: Vec<&str> = self.content.lines().collect();
                let first = files
                    .first()
                    .and_then(|f| f.rsplit(['\\', '/']).next())
                    .unwrap_or_default();
                if files.len() > 1 {
                    format!("{first} 等 {} 个文件", files.len())
                } else {
                    first.to_string()
                }
            }
            Kind::Image => self.content.to_string(),
            _ => self
                .content
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or_default()
                .to_string(),
        }
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
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        rest.split('/').next().unwrap_or(rest)
    }
}

pub fn sample_clips() -> Vec<Clip> {
    vec![
        Clip {
            kind: Kind::Text,
            content: "验证码 482913，5 分钟内有效，请勿泄露给他人。",
            meta: "",
            source: "微信",
            ago: "刚刚",
            pinned: false,
        },
        Clip {
            kind: Kind::Link,
            content: "https://github.com/longbridge/gpui-kit",
            meta: "",
            source: "Microsoft Edge",
            ago: "2 分钟前",
            pinned: true,
        },
        Clip {
            kind: Kind::Code,
            content: "fn planned_search(conn: &Connection, query: &str) -> Result<Vec<i64>> {\n    let (long, short): (Vec<&str>, Vec<&str>) =\n        query.split_whitespace().partition(|t| t.chars().count() >= 3);\n    // 长词走 FTS5 trigram，短词走 LIKE\n    todo!()\n}",
            meta: "Rust",
            source: "Visual Studio Code",
            ago: "8 分钟前",
            pinned: false,
        },
        Clip {
            kind: Kind::Color,
            content: "#4F6BED",
            meta: "",
            source: "Figma",
            ago: "15 分钟前",
            pinned: false,
        },
        Clip {
            kind: Kind::Image,
            content: "截图 2026-11-24 09.41.12",
            meta: "1920 × 1080 · PNG · 284 KB",
            source: "截图工具",
            ago: "32 分钟前",
            pinned: false,
        },
        Clip {
            kind: Kind::Text,
            content: "收货地址：上海市浦东新区世纪大道 100 号 环球金融中心 32 层，联系人 李女士，电话 138 0000 0000",
            meta: "",
            source: "钉钉",
            ago: "1 小时前",
            pinned: true,
        },
        Clip {
            kind: Kind::File,
            content: "D:\\项目\\设计稿\\首页-v3.fig\nD:\\项目\\设计稿\\图标集.svg\nD:\\项目\\设计稿\\配色方案.pdf",
            meta: "",
            source: "文件资源管理器",
            ago: "2 小时前",
            pinned: false,
        },
        Clip {
            kind: Kind::Text,
            content: "会议纪要：周三下午 3 点评审 M1 交付范围。\n1. 粘贴队列放进 M2\n2. 同步协议先做桌面 ↔ 桌面\n3. 下周确定正式项目名",
            meta: "",
            source: "飞书",
            ago: "昨天",
            pinned: false,
        },
        Clip {
            kind: Kind::Link,
            content: "https://www.iroh.computer/blog/v1",
            meta: "",
            source: "Microsoft Edge",
            ago: "昨天",
            pinned: false,
        },
        Clip {
            kind: Kind::Code,
            content: "SELECT id, content FROM clip_items\nWHERE kind = 'text'\nORDER BY seq DESC\nLIMIT 50;",
            meta: "SQL",
            source: "DataGrip",
            ago: "昨天",
            pinned: false,
        },
        Clip {
            kind: Kind::Color,
            content: "#10B981",
            meta: "",
            source: "Figma",
            ago: "2 天前",
            pinned: false,
        },
        Clip {
            kind: Kind::Text,
            content: "The quick brown fox jumps over the lazy dog. 😀 Emoji 也要能正确粘贴 🎉",
            meta: "",
            source: "记事本",
            ago: "3 天前",
            pinned: false,
        },
    ]
}
