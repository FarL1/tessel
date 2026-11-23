//! Tessel 的核心：数据模型、内容识别、存储与搜索。
//!
//! 这个 crate **不依赖任何 UI 框架和平台 API**：桌面端、CLI、将来的移动端都复用它。

pub mod classify;
pub mod model;
pub mod store;
pub mod time;

pub use model::{Clip, Filter, ImageInfo, Kind, NewClip, Payload};
pub use store::{Counts, Store};
