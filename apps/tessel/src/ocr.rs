//! 图片文字识别（OCR）：用 Windows 自带的 `Windows.Media.Ocr`，完全离线，不需要额外的模型文件。
//!
//! 后台线程：启动时把历史里还没识别过的图片补上，之后每采集到一张新图片就识别一次。
//! 识别结果存进数据库（`Store::set_ocr`），图片里的文字就能被搜索。
//! 识别引擎使用系统的显示语言（中文系统要装了中文语言包，一般默认就有）。

use std::path::PathBuf;
use std::sync::mpsc;

use tessel_core::Store;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

use crate::{AppCommand, debug_log, log_line};

/// 启动识别线程，返回用来唤醒它的发送端（有新图片时发一个 `()`）
pub fn spawn(dir: PathBuf, tx: async_channel::Sender<AppCommand>) -> Option<mpsc::Sender<()>> {
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("tessel-ocr".into())
        .spawn(move || {
            use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
            let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
                Ok(engine) => engine,
                Err(err) => {
                    log_line(format!(
                        "[ocr] 系统没有可用的文字识别引擎，不识别图片：{err}"
                    ));
                    return;
                }
            };
            let store = match Store::open(&dir) {
                Ok(store) => store,
                Err(err) => {
                    log_line(format!("[ocr] 无法打开数据库：{err}"));
                    return;
                }
            };
            loop {
                // 处理完所有待识别的图片，再等下一次唤醒
                while let Ok(pending) = store.images_pending_ocr(20) {
                    if pending.is_empty() {
                        break;
                    }
                    for (uuid, path) in pending {
                        let t0 = std::time::Instant::now();
                        let text = std::fs::read(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|png| recognize(&engine, &png).map_err(|e| e.to_string()));
                        // 失败也记为"识别过"（空文字），不会反复重试同一张图
                        let text = text.unwrap_or_else(|err| {
                            debug_log(format!("[ocr] 识别失败：{err}"));
                            String::new()
                        });
                        debug_log(format!(
                            "[ocr] {} 字 · {:.0}ms",
                            text.chars().count(),
                            t0.elapsed().as_secs_f64() * 1000.0
                        ));
                        if store.set_ocr(&uuid, &text).is_ok() && !text.is_empty() {
                            let _ = tx.try_send(AppCommand::ClipboardChanged { notify: false });
                        }
                    }
                }
                if wake_rx.recv().is_err() {
                    break;
                }
                while wake_rx.try_recv().is_ok() {}
            }
        })
        .ok()?;
    Some(wake_tx)
}

/// 识别一张 PNG 里的文字，每行一行
fn recognize(engine: &OcrEngine, png: &[u8]) -> windows::core::Result<String> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(png)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    let decoder =
        BitmapDecoder::CreateWithIdAsync(BitmapDecoder::PngDecoderId()?, &stream)?.join()?;
    let bitmap = decoder
        .GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)?
        .join()?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let lines = result.Lines()?;
    let mut out = Vec::new();
    for i in 0..lines.Size()? {
        let line = tidy_cjk(&lines.GetAt(i)?.Text()?.to_string());
        if !line.trim().is_empty() {
            out.push(line);
        }
    }
    Ok(out.join("\n"))
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F | 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// 识别中文时，引擎会在每个字之间加空格（"发 票 号 码"）：去掉两个中日文字符之间的空格
fn tidy_cjk(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' && i > 0 && i + 1 < chars.len() && is_cjk(chars[i - 1]) && is_cjk(chars[i + 1])
        {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_spacing() {
        assert_eq!(tidy_cjk("发 票 号 码 INV-0042"), "发票号码 INV-0042");
        assert_eq!(tidy_cjk("Hello world"), "Hello world");
        assert_eq!(tidy_cjk("金 额 ： 128.00 元"), "金额： 128.00 元");
    }
}
