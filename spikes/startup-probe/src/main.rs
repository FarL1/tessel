//! 拆分 GPUI 在 Windows 上的启动开销：DirectWrite 系统字体集合、D3D11 设备创建。
//! 运行：`cargo run -p spike-startup-probe --release`

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use std::time::Instant;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
    };
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory5, IDWriteFontCollection1,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIFactory6,
    };

    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let factory: IDWriteFactory5 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
    println!(
        "DWriteCreateFactory                         {:>8.1}ms",
        ms(t)
    );

    for check_updates in [true, false, true] {
        let t = Instant::now();
        let mut coll: Option<IDWriteFontCollection1> = None;
        unsafe { factory.GetSystemFontCollection(false, &mut coll, check_updates)? };
        let n = unsafe { coll.as_ref().map(|c| c.GetFontFamilyCount()).unwrap_or(0) };
        println!(
            "GetSystemFontCollection(checkForUpdates={check_updates:<5}) {:>8.1}ms  ({n} 个字体族)",
            ms(t)
        );
    }

    let t = Instant::now();
    let dxgi: IDXGIFactory6 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
    println!(
        "CreateDXGIFactory2                          {:>8.1}ms",
        ms(t)
    );
    let adapter = unsafe { dxgi.EnumAdapters1(0)? };

    for i in 0..2 {
        let t = Instant::now();
        let mut device = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?
        };
        println!(
            "D3D11CreateDevice（第 {} 次）                {:>8.1}ms",
            i + 1,
            ms(t)
        );
    }
    Ok(())
}
