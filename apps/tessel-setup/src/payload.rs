//! 安装包本体：Velopack 生成的 Setup.exe 附加在本程序 exe 的末尾（scripts/package.ps1 负责拼接）。
//!
//! 文件布局：`[tessel-setup.exe][Setup.exe 的字节][长度：u64 小端][MAGIC：8 字节]`
//!
//! 开发时可以用环境变量 `TESSEL_SETUP_PAYLOAD=<Setup.exe 路径>` 直接指定，不用拼接。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tessel_core::i18n::tr;
use tessel_core::trf;

pub const MAGIC: &[u8; 8] = b"TSLPAYLD";
const TAIL: u64 = 16;

pub enum Source {
    /// 附加在 exe 末尾：从 `offset` 开始的 `len` 个字节
    Embedded { exe: PathBuf, offset: u64, len: u64 },
    /// 外部文件（开发时用）
    File(PathBuf),
}

/// 解析 exe 末尾的 16 字节，返回安装包的起始位置和长度
fn parse_tail(file_len: u64, tail: &[u8; 16]) -> Option<(u64, u64)> {
    if &tail[8..] != MAGIC {
        return None;
    }
    let len = u64::from_le_bytes(tail[..8].try_into().ok()?);
    let offset = file_len.checked_sub(TAIL)?.checked_sub(len)?;
    (len > 0).then_some((offset, len))
}

/// 找到安装包
pub fn locate() -> Result<Source, String> {
    if let Some(path) = std::env::var_os("TESSEL_SETUP_PAYLOAD") {
        return Ok(Source::File(path.into()));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut file = File::open(&exe).map_err(|e| e.to_string())?;
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut tail = [0u8; 16];
    if file_len >= TAIL {
        file.seek(SeekFrom::End(-(TAIL as i64)))
            .and_then(|_| file.read_exact(&mut tail))
            .map_err(|e| e.to_string())?;
    }
    match parse_tail(file_len, &tail) {
        Some((offset, len)) => Ok(Source::Embedded { exe, offset, len }),
        None => Err(tr(
            "这个安装程序里没有找到安装包，可能下载不完整，请重新下载。",
            "The installer package is missing from this file. It may be incomplete; please download it again.",
        )
        .into()),
    }
}

/// 把安装包放到临时目录，返回可以运行的 Setup.exe 路径
pub fn extract(source: &Source) -> Result<PathBuf, String> {
    match source {
        Source::File(path) => Ok(path.clone()),
        Source::Embedded { exe, offset, len } => {
            let dir = temp_dir();
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let out_path = dir.join("Tessel.App-win-Setup.exe");
            let mut input = File::open(exe).map_err(|e| e.to_string())?;
            input
                .seek(SeekFrom::Start(*offset))
                .map_err(|e| e.to_string())?;
            let mut out = File::create(&out_path).map_err(|e| e.to_string())?;
            let copied = std::io::copy(&mut input.take(*len), &mut out)
                .map_err(|e| trf!("解压安装包失败：{e}", "Could not unpack the installer: {e}"))?;
            if copied != *len {
                return Err(tr(
                    "安装包不完整，请重新下载。",
                    "The installer package is incomplete. Please download it again.",
                )
                .into());
            }
            Ok(out_path)
        }
    }
}

/// 删除临时目录（只删我们自己解压出来的）
pub fn cleanup(source: &Source, extracted: &Path) {
    if let Source::Embedded { .. } = source
        && let Some(dir) = extracted.parent()
        && dir == temp_dir()
    {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("tessel-setup-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(len: u64, magic: &[u8; 8]) -> [u8; 16] {
        let mut t = [0u8; 16];
        t[..8].copy_from_slice(&len.to_le_bytes());
        t[8..].copy_from_slice(magic);
        t
    }

    #[test]
    fn finds_appended_payload() {
        // 1000 字节的程序 + 500 字节的安装包 + 16 字节尾部
        assert_eq!(parse_tail(1516, &tail(500, MAGIC)), Some((1000, 500)));
    }

    #[test]
    fn rejects_bad_tails() {
        assert_eq!(parse_tail(1516, &tail(500, b"XXXXXXXX")), None);
        // 长度比文件还大
        assert_eq!(parse_tail(100, &tail(500, MAGIC)), None);
        assert_eq!(parse_tail(1516, &tail(0, MAGIC)), None);
    }
}
