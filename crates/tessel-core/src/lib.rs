//! Tessel 的核心：数据模型、内容识别、存储与搜索。
//!
//! 这个 crate **不依赖任何 UI 框架和平台 API**：桌面端、CLI、将来的移动端都复用它。

pub mod classify;
pub mod config;
pub mod model;
pub mod pinyin_index;
pub mod query;
pub mod store;
pub mod time;
pub mod transform;

pub use config::{Config, ThemePref};
pub use model::{Clip, Filter, ImageInfo, Kind, NewClip, Payload, RichFormats};
pub use query::Query;
pub use store::{Counts, Store};
