//! 平台剪贴板：事件驱动的监听、遵守隐私标记的读取、写回剪贴板。
//!
//! 目前只实现了 Windows（M1 首发平台）。macOS / Linux 在 M5 实现同样的接口。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tessel_core::NewClip;

mod image_conv;
#[cfg(windows)]
mod windows;

/// 启动后台监听线程。每次剪贴板变化（防抖合并后）调用一次 `on_capture`。
///
/// - `paused` 为 true 时忽略所有变化（"暂停记录"）
/// - `is_ignored(exe 路径)` 返回 true 时跳过（"忽略的应用"），在读取内容之前判断
/// - 带隐私标记的内容（密码管理器等）直接跳过，不会读取内容
/// - 回调在监听线程上执行，不要在里面做耗时操作以外的阻塞等待
pub fn spawn_watcher(
    paused: Arc<AtomicBool>,
    is_ignored: impl Fn(&str) -> bool + Send + 'static,
    on_capture: impl Fn(NewClip) + Send + 'static,
) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows::spawn_watcher(paused, is_ignored, on_capture)
    }
    #[cfg(not(windows))]
    {
        let _ = (paused, is_ignored, on_capture);
        Err(std::io::Error::other(
            "clipboard watcher not implemented on this platform",
        ))
    }
}

/// 写回剪贴板的内容
pub enum WriteContent<'a> {
    Text(&'a str),
    /// 纯文本 + 原样保存的富格式（HTML Format / Rich Text Format 的原始字节）
    RichText {
        text: &'a str,
        html: Option<&'a [u8]>,
        rtf: Option<&'a [u8]>,
    },
    Files(Vec<&'a str>),
    /// PNG 编码的图片
    ImagePng(&'a [u8]),
}

/// 没有窗口的程序（命令行工具）写入剪贴板
pub fn write_standalone(content: WriteContent) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::write_standalone(content)
    }
    #[cfg(not(windows))]
    {
        let _ = content;
        Err("not implemented on this platform".into())
    }
}

/// 写入系统剪贴板。`owner` 是剪贴板所有者窗口（可选）。
pub fn write(owner: Option<isize>, content: WriteContent) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::write(owner, content)
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, content);
        Err("not implemented on this platform".into())
    }
}
