//! 和系统打交道的部分：安装目录、已安装版本、开机自启、运行 Velopack 安装程序。
//!
//! 测试时设置 `TESSEL_SETUP_DRYRUN=1`（或 `fail`）只模拟安装，不运行安装程序、
//! 不改注册表、不启动应用，不会影响这台电脑上已经装好的 Tessel。

use std::path::PathBuf;
use std::time::Duration;

use tessel_core::i18n::tr;
use tessel_core::trf;

/// Velopack 的安装目录（packId = Tessel.App）
pub fn install_dir() -> PathBuf {
    local_app_data().join("Tessel.App")
}

/// 用户数据目录（历史、设置），和安装目录分开，卸载时保留
pub fn data_dir() -> PathBuf {
    local_app_data().join("Tessel")
}

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn app_exe() -> PathBuf {
    install_dir().join("current").join("tessel.exe")
}

/// Velopack 安装日志
pub fn velopack_log() -> PathBuf {
    local_app_data().join("velopack").join("velopack.log")
}

/// 已经安装的版本（读 Velopack 写的 current\sq.version）
pub fn installed_version() -> Option<String> {
    let text = std::fs::read_to_string(install_dir().join("current").join("sq.version")).ok()?;
    let start = text.find("<version>")? + "<version>".len();
    let end = start + text[start..].find("</version>")?;
    Some(text[start..end].trim().to_string()).filter(|v| !v.is_empty())
}

/// 模拟安装：`Some(true)` 模拟成功，`Some(false)` 模拟失败
pub fn dry_run() -> Option<bool> {
    match std::env::var("TESSEL_SETUP_DRYRUN").as_deref() {
        Ok("fail") => Some(false),
        Ok(_) => Some(true),
        Err(_) => None,
    }
}

/// 运行 Velopack 的安装程序（静默模式：不弹它自己的对话框，也不在装完后自动启动应用）
pub fn run_velopack(setup: &std::path::Path) -> Result<(), String> {
    if let Some(ok) = dry_run() {
        std::thread::sleep(Duration::from_millis(2500));
        return if ok {
            Ok(())
        } else {
            Err("模拟的安装失败（TESSEL_SETUP_DRYRUN=fail）".into())
        };
    }
    let mut cmd = std::process::Command::new(setup);
    cmd.arg("--silent");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let status = cmd
        .status()
        .map_err(|e| trf!("无法运行安装程序：{e}", "Could not run the installer: {e}"))?;
    if status.success() && app_exe().exists() {
        Ok(())
    } else {
        Err(match status.code() {
            Some(code) => trf!(
                "安装程序出错（代码 {code}）。",
                "The installer failed (code {code})."
            ),
            None => tr("安装程序意外退出。", "The installer exited unexpectedly.").into(),
        })
    }
}

/// 启动装好的 Tessel（不带参数：显示剪贴板历史窗口）
pub fn launch_app() -> Result<(), String> {
    if dry_run().is_some() {
        return Ok(());
    }
    let exe = app_exe();
    std::process::Command::new(&exe)
        .current_dir(exe.parent().unwrap_or(&install_dir()))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 用系统默认程序打开文件或文件夹
pub fn open_path(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

/// Tessel 是否正在运行（正式实例会创建这个命名事件）
pub fn tessel_running() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};
        use windows::core::HSTRING;
        let name = HSTRING::from(tessel_core::instance::Request::Show.event_name());
        match unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &name) } {
            Ok(handle) => {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(handle);
                }
                true
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    false
}

/// Windows 显示语言是不是中文
pub fn system_is_chinese() -> bool {
    #[cfg(windows)]
    {
        let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        lang & 0x3FF == 0x04
    }
    #[cfg(not(windows))]
    false
}

// 开机自启：和 Tessel 自己（设置里的开关）写同一个注册表值、同样的命令，两边互相认得
#[cfg(windows)]
const RUN_KEY: windows::core::PCWSTR =
    windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
#[cfg(windows)]
const RUN_VALUE: windows::core::PCWSTR = windows::core::w!("Tessel");

fn autostart_command() -> String {
    format!("\"{}\" --background", app_exe().display())
}

/// 是否已经登记了开机自启（更新时沿用用户原来的选择）
pub fn autostart_registered() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                RRF_RT_REG_SZ,
                None,
                None,
                None,
            )
        };
        err.is_ok()
    }
    #[cfg(not(windows))]
    false
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    // 模拟安装、测试实例都不改：这个注册表值只有一个，会影响正式安装的 Tessel
    if dry_run().is_some() || std::env::var_os("TESSEL_DATA_DIR").is_some() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{
            HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
        };
        let err = if enabled {
            let wide: Vec<u16> = autostart_command()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    RUN_KEY,
                    RUN_VALUE,
                    REG_SZ.0,
                    Some(wide.as_ptr() as *const _),
                    (wide.len() * 2) as u32,
                )
            }
        } else {
            let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) };
            if err == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            err
        };
        err.ok().map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
}
