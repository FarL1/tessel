//! 把品牌 SVG 渲染成各尺寸 PNG 和多尺寸 Windows .ico。
//!
//! 运行：`cargo run -p icon-gen --release`（在仓库根目录）
//! - ≤32px 用简化版 `tessel-small.svg`，其余用 `tessel.svg`
//! - 输出到 `assets/brand/png/` 和 `assets/brand/tessel.ico`

use std::fs;
use std::path::Path;

use resvg::{tiny_skia, usvg};

const SMALL_MAX: u32 = 32;
const PNG_SIZES: &[u32] = &[16, 20, 24, 32, 40, 48, 64, 128, 256, 512];
const ICO_SIZES: &[u32] = &[16, 20, 24, 32, 40, 48, 64, 256];

fn render(svg: &[u8], size: u32) -> tiny_skia::Pixmap {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).expect("parse svg");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
}

/// tiny-skia 的像素是预乘 alpha 的，.ico 需要普通 RGBA
fn straight_rgba(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

fn main() {
    let dir = Path::new("assets/brand");
    let full = fs::read(dir.join("tessel.svg")).expect("read tessel.svg");
    let small = fs::read(dir.join("tessel-small.svg")).expect("read tessel-small.svg");
    let pick = |size: u32| if size <= SMALL_MAX { &small } else { &full };

    fs::create_dir_all(dir.join("png")).expect("create png dir");
    for &size in PNG_SIZES {
        let path = dir.join("png").join(format!("tessel-{size}.png"));
        render(pick(size), size).save_png(&path).expect("save png");
        println!("png  {}", path.display());
    }

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in ICO_SIZES {
        let pixmap = render(pick(size), size);
        let image = ico::IconImage::from_rgba_data(size, size, straight_rgba(&pixmap));
        icon_dir.add_entry(ico::IconDirEntry::encode(&image).expect("encode ico entry"));
    }
    let ico_path = dir.join("tessel.ico");
    icon_dir
        .write(fs::File::create(&ico_path).expect("create ico"))
        .expect("write ico");
    println!("ico  {} ({} sizes)", ico_path.display(), ICO_SIZES.len());
}
