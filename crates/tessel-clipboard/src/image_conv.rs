//! 图片格式转换：Windows DIB ⇄ PNG。

use std::io::Cursor;

use image::{ImageFormat, ImageReader, RgbaImage};

/// 读取 PNG 的尺寸（只解析文件头，不解码像素）
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    ImageReader::with_format(Cursor::new(png), ImageFormat::Png)
        .into_dimensions()
        .ok()
}

/// 把剪贴板里的 DIB（`CF_DIB` / `CF_DIBV5`，即不带文件头的 BMP）转成 PNG。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn dib_to_png(dib: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if dib.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes(dib[o..o + 4].try_into().unwrap());
    let header_size = u32_at(0);
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]) as u32;
    let compression = u32_at(16);
    let colors_used = u32_at(32);
    // 调色板项数；BITMAPINFOHEADER + BI_BITFIELDS 时三个颜色掩码跟在头后面
    let palette = if colors_used > 0 {
        colors_used
    } else if bit_count <= 8 {
        1 << bit_count
    } else {
        0
    };
    let masks = match (header_size, compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    };
    let offset = 14 + header_size + masks + palette * 4;

    // 补上 14 字节的 BITMAPFILEHEADER，交给 BMP 解码器
    let mut bmp = Vec::with_capacity(dib.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&offset.to_le_bytes());
    bmp.extend_from_slice(dib);

    let mut rgba = image::load_from_memory_with_format(&bmp, ImageFormat::Bmp)
        .ok()?
        .to_rgba8();
    // 很多程序写入的 32 位 DIB 的 alpha 全是 0（并非真的透明），按不透明处理
    if bit_count == 32 && rgba.pixels().all(|p| p.0[3] == 0) {
        for p in rgba.pixels_mut() {
            p.0[3] = 255;
        }
    }
    let (w, h) = rgba.dimensions();
    let mut png = Vec::new();
    rgba.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    Some((png, w, h))
}

/// PNG → 32 位自下而上的 DIB（BITMAPINFOHEADER + BGRA 像素），用于写回 `CF_DIB`
#[cfg_attr(not(windows), allow(dead_code))]
pub fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    let rgba: RgbaImage = image::load_from_memory_with_format(png, ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut dib = Vec::with_capacity(40 + (w * h * 4) as usize);
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&(h as i32).to_le_bytes()); // biHeight（正数 = 自下而上）
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&(w * h * 4).to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&[0u8; 16]); // 分辨率、调色板：都填 0
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = rgba.get_pixel(x, y).0;
            dib.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(dib)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_roundtrip() {
        let mut img = RgbaImage::new(3, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(2, 1, image::Rgba([0, 0, 255, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();

        assert_eq!(png_dimensions(&png), Some((3, 2)));
        let dib = png_to_dib(&png).unwrap();
        let (png2, w, h) = dib_to_png(&dib).unwrap();
        assert_eq!((w, h), (3, 2));
        let back = image::load_from_memory(&png2).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(back.get_pixel(2, 1).0, [0, 0, 255, 255]);
    }

    #[test]
    fn zero_alpha_dib_becomes_opaque() {
        // 1×1 的 32 位 DIB，像素 BGRA = (10, 20, 30, 0)
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        dib.extend_from_slice(&[10, 20, 30, 0]);
        let (png, ..) = dib_to_png(&dib).unwrap();
        let px = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(px.get_pixel(0, 0).0, [30, 20, 10, 255]);
    }
}
