//! 进程间通知用的名字：桌面端（单实例）和命令行工具（`tessel-cli show` 等）共用。
//!
//! 用命名事件 `Local\Tessel.<请求><实例键>`，不开端口、不建管道。
//! 指定了 `TESSEL_DATA_DIR` 时按数据目录区分实例，测试实例不会和正式实例互相干扰。

/// 可以发给正在运行的 Tessel 的请求
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// 显示弹窗
    Show,
    /// 打开设置窗口
    Settings,
    /// 粘贴队列里的下一条（和队列热键一样）
    PasteNext,
}

impl Request {
    pub const ALL: [Request; 3] = [Request::Show, Request::Settings, Request::PasteNext];

    fn kind(self) -> &'static str {
        match self {
            Request::Show => "ShowRequest",
            Request::Settings => "SettingsRequest",
            Request::PasteNext => "PasteNextRequest",
        }
    }

    /// 命名事件的名字
    pub fn event_name(self) -> String {
        format!(r"Local\Tessel.{}{}", self.kind(), instance_key())
    }
}

/// 实例键：默认数据目录为空，`TESSEL_DATA_DIR` 指定的目录按路径取哈希
pub fn instance_key() -> String {
    match std::env::var_os("TESSEL_DATA_DIR") {
        Some(dir) => format!(".{:016x}", stable_hash(dir.to_string_lossy().as_bytes())),
        None => String::new(),
    }
}

/// FNV-1a：结果在任何 Rust 版本下都稳定（标准库的 DefaultHasher 不保证）
fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_stable() {
        // 已经发布的版本用的就是这个算法和名字，不能改
        // FNV-1a 64 位的标准测试向量
        assert_eq!(stable_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert!(
            Request::Show
                .event_name()
                .starts_with(r"Local\Tessel.ShowRequest")
        );
        assert!(
            Request::PasteNext
                .event_name()
                .starts_with(r"Local\Tessel.PasteNextRequest")
        );
    }
}
