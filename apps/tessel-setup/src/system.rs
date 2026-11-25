//! 和系统打交道的部分：安装位置、已安装版本、开机自启、运行 Velopack 的安装 / 卸载。
//!
//! 测试时设置 `TESSEL_SETUP_DRYRUN=1`（或 `fail`）只模拟：不运行安装 / 卸载程序、
//! 不改注册表、不删文件、不启动应用，不会影响这台电脑上已经装好的 Tessel。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tessel_core::i18n::tr;
use tessel_core::trf;

/// 卸载程序的文件名（放在安装目录里，"设置 → 应用"里的卸载入口指向它）
pub const UNINSTALLER: &str = "Uninstall Tessel.exe";
/// Velopack 登记的卸载信息（packId = Tessel.App）
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessel.App";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Tessel";

/// 默认安装位置：Velopack 的默认目录
pub fn default_install_dir() -> PathBuf {
    local_app_data().join("Tessel.App")
}

/// 用户数据目录（历史、设置），和安装目录分开
pub fn data_dir() -> PathBuf {
    local_app_data().join("Tessel")
}

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn app_exe(dir: &Path) -> PathBuf {
    dir.join("current").join("tessel.exe")
}

/// Velopack 的安装 / 卸载日志
pub fn velopack_log() -> PathBuf {
    local_app_data().join("velopack").join("velopack.log")
}

fn is_tessel_install(dir: &Path) -> bool {
    dir.join("Update.exe").is_file() || dir.join("current").join("sq.version").is_file()
}

/// 已经安装在哪里：先看卸载信息里登记的位置（可能是用户自选的），再看默认位置
pub fn existing_install() -> Option<PathBuf> {
    // 模拟模式下可以假装没装过，用来查看全新安装的界面
    if dry_run().is_some() && std::env::var_os("TESSEL_SETUP_FRESH").is_some() {
        return None;
    }
    reg_get(UNINSTALL_KEY, "InstallLocation")
        .map(PathBuf::from)
        .filter(|d| is_tessel_install(d))
        .or_else(|| Some(default_install_dir()).filter(|d| is_tessel_install(d)))
}

/// 已经安装的版本（读 Velopack 写的 current\sq.version）
pub fn installed_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("current").join("sq.version")).ok()?;
    let start = text.find("<version>")? + "<version>".len();
    let end = start + text[start..].find("</version>")?;
    Some(text[start..end].trim().to_string()).filter(|v| !v.is_empty())
}

/// 用户选的文件夹 → 实际安装目录。
/// Velopack 会把安装目录整个当成自己的（重装时改名、清空），所以总是装进一个单独的
/// `Tessel` 子文件夹，除非用户选的就是这样一个文件夹。
pub fn install_target(picked: &Path) -> PathBuf {
    let name = picked
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name == "tessel" || name == "tessel.app" || is_tessel_install(picked) {
        picked.to_path_buf()
    } else {
        picked.join("Tessel")
    }
}

fn lower(p: &Path) -> String {
    p.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}

/// 检查能不能装到这个目录
pub fn check_target(dir: &Path) -> Result<(), String> {
    if !dir.is_absolute() || dir.parent().is_none() {
        return Err(tr(
            "不能直接装在磁盘根目录，请选一个文件夹。",
            "Pick a folder rather than the root of a drive.",
        )
        .into());
    }
    // 不能和数据目录重叠：卸载时会删除整个安装目录
    let (d, data) = (lower(dir), lower(&data_dir()));
    let inside = |a: &str, b: &str| a == b || a.starts_with(&format!("{b}\\"));
    if inside(&d, &data) || inside(&data, &d) {
        return Err(trf!(
            "这个位置和 Tessel 的数据目录（{}）重叠，卸载时会连历史一起删掉，请换一个文件夹。",
            "This overlaps Tessel's data folder ({}). Uninstalling would delete your history, so pick another folder.",
            data_dir().display()
        ));
    }
    // 已有其他文件的文件夹：Velopack 会清空它。
    // 默认位置（Velopack 专用的 Tessel.App）、以及里面有我们的卸载程序的文件夹（上次卸载的残留）除外。
    let ours = lower(dir) == lower(&default_install_dir()) || dir.join(UNINSTALLER).is_file();
    if dir.is_dir() && !is_tessel_install(dir) && !ours {
        let has_files = std::fs::read_dir(dir)
            .map(|mut it| it.next().is_some())
            .unwrap_or(true);
        if has_files {
            return Err(trf!(
                "「{}」里已经有其他文件，安装时会被清空。请选一个空文件夹或新位置。",
                "\"{}\" already contains other files, which setup would remove. Pick an empty or new folder.",
                dir.display()
            ));
        }
    }
    // 不用管理员权限能不能写（Program Files 之类的位置不行）
    let mut probe_dir = dir;
    while !probe_dir.exists() {
        match probe_dir.parent() {
            Some(p) => probe_dir = p,
            None => break,
        }
    }
    let probe = probe_dir.join(format!(".tessel-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => Err(tr(
            "这个位置需要管理员权限才能写入。Tessel 按当前用户安装，请换一个文件夹（比如 D:\\Apps）。",
            "Writing here needs administrator rights. Tessel installs per user, so pick another folder (for example D:\\Apps).",
        )
        .into()),
    }
}

/// 模拟模式：`Some(true)` 模拟成功，`Some(false)` 模拟失败
pub fn dry_run() -> Option<bool> {
    match std::env::var("TESSEL_SETUP_DRYRUN").as_deref() {
        Ok("fail") => Some(false),
        Ok(_) => Some(true),
        Err(_) => None,
    }
}

/// 模拟模式下记录"本来会做的事"，测试时检查（%TEMP% 下的 tessel-setup-dryrun.log）
fn dry_log(line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("tessel-setup-dryrun.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

fn simulate(what: &str) -> Result<(), String> {
    dry_log(&format!("模拟{what}"));
    std::thread::sleep(Duration::from_millis(2500));
    match dry_run() {
        Some(false) => Err(format!("模拟的{what}失败（TESSEL_SETUP_DRYRUN=fail）")),
        _ => Ok(()),
    }
}

fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn exit_error(status: std::process::ExitStatus, zh: &str, en: &str) -> String {
    let what = tr_dyn(zh, en);
    match status.code() {
        Some(code) => trf!(
            "{what}出错（代码 {code}）。",
            "The {what} failed (code {code})."
        ),
        None => trf!("{what}意外退出。", "The {what} exited unexpectedly."),
    }
}

fn tr_dyn<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if tessel_core::i18n::is_en() { en } else { zh }
}

/// 运行 Velopack 的安装程序：静默模式（不弹它自己的对话框，装完也不自动启动应用），装到 `dir`
pub fn run_velopack(setup: &Path, dir: &Path) -> Result<(), String> {
    if dry_run().is_some() {
        return simulate("安装");
    }
    // 自选位置的上级文件夹可能还不存在（安装目录本身由 Velopack 创建）
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| trf!("无法创建文件夹：{e}", "Could not create the folder: {e}"))?;
    }
    let status = hidden(
        std::process::Command::new(setup)
            .arg("--silent")
            .arg("--installto")
            .arg(dir),
    )
    .status()
    .map_err(|e| trf!("无法运行安装程序：{e}", "Could not run the installer: {e}"))?;
    if !status.success() {
        Err(exit_error(status, "安装程序", "installer"))
    } else if !app_exe(dir).exists() {
        Err(trf!(
            "安装程序已经结束，但没有找到 {}。",
            "Setup finished, but {} is missing.",
            app_exe(dir).display()
        ))
    } else {
        Ok(())
    }
}

/// 把卸载程序放进安装目录，并让"设置 → 应用"里的卸载入口指向它
pub fn install_uninstaller(dir: &Path) -> Result<(), String> {
    if dry_run().is_some() {
        return Ok(());
    }
    crate::payload::write_program(&dir.join(UNINSTALLER))?;
    let cmd = format!("\"{}\" --uninstall", dir.join(UNINSTALLER).display());
    reg_set(UNINSTALL_KEY, "UninstallString", &cmd)
}

/// 运行 Velopack 的卸载（删除程序、快捷方式、卸载信息）
pub fn run_uninstall(dir: &Path) -> Result<(), String> {
    if dry_run().is_some() {
        return simulate("卸载");
    }
    let update = dir.join("Update.exe");
    let status = hidden(
        std::process::Command::new(&update)
            .arg("--uninstall")
            .arg("--silent"),
    )
    .status()
    .map_err(|e| {
        trf!(
            "无法运行卸载程序：{e}",
            "Could not run the uninstaller: {e}"
        )
    })?;
    // 开机自启：Tessel 的卸载钩子会关掉，这里再确认一次
    let _ = set_autostart(dir, false);
    if status.success() {
        Ok(())
    } else {
        Err(exit_error(status, "卸载程序", "uninstaller"))
    }
}

/// 删除剪贴板历史和设置（只删数据目录本身）
pub fn delete_data() -> Result<(), String> {
    if dry_run().is_some() {
        return Ok(());
    }
    let dir = data_dir();
    if !dir.exists() {
        return Ok(());
    }
    // Tessel 刚被结束，数据库文件可能还没完全释放，稍等重试
    let mut last = String::new();
    for _ in 0..10 {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Err(trf!(
        "程序已卸载，但删除数据目录失败：{last}。可以手动删除 {}",
        "Tessel was removed, but its data folder could not be deleted: {last}. You can delete {} by hand.",
        dir.display()
    ))
}

/// 启动装好的 Tessel（不带参数：显示剪贴板历史窗口），并等它的窗口出现。
///
/// 安装程序退出前把"设置前台窗口"的权限交给 Tessel：否则 Windows 不让后台启动的程序抢前台，
/// Tessel 的窗口虽然显示了，键盘焦点却不在它上面（按 Esc 没反应）。
pub fn launch_app(dir: &Path) -> Result<(), String> {
    if dry_run().is_some() {
        dry_log(&format!("启动 {}", app_exe(dir).display()));
        return Ok(());
    }
    allow_foreground();
    let exe = app_exe(dir);
    let child = std::process::Command::new(&exe)
        .current_dir(exe.parent().unwrap_or(dir))
        .spawn()
        .map_err(|e| e.to_string())?;
    // 在 Tessel 的窗口出现之前不退出：安装程序还是前台程序时，它启动的程序才能拿到前台
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !has_visible_window(child.id()) {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// 用系统默认程序打开文件或文件夹
pub fn open_path(path: &Path) {
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

pub fn allow_foreground() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
}

#[cfg(windows)]
fn has_visible_window(pid: u32) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };
    use windows::core::BOOL;
    struct Search {
        pid: u32,
        found: bool,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == search.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            search.found = true;
            return false.into();
        }
        true.into()
    }
    let mut search = Search { pid, found: false };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

#[cfg(not(windows))]
fn has_visible_window(_pid: u32) -> bool {
    true
}

/// 临时目录里的那份卸载程序带这个参数启动
const FROM_TEMP: &str = "--from-temp";

fn is_temp_copy() -> bool {
    std::env::args().any(|a| a == FROM_TEMP)
}

/// 卸载程序在安装目录里运行时，先把自己复制到临时目录再运行：
/// 卸载要删除整个安装目录，正在运行的 exe 删不掉。返回 true 表示已经转交，当前进程应退出。
/// （用参数而不是比较路径来判断"已经在临时目录里"：路径可能有短文件名等不同写法，比较不可靠）
pub fn relaunch_from_temp_if_needed() -> bool {
    if is_temp_copy() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let temp = std::env::temp_dir();
    let copy = temp.join(format!("tessel-uninstall-{}.exe", std::process::id()));
    if std::fs::copy(&exe, &copy).is_err() {
        return false;
    }
    allow_foreground();
    std::process::Command::new(&copy)
        .args(["--uninstall", FROM_TEMP])
        .current_dir(&temp)
        .spawn()
        .is_ok()
}

/// 卸载程序退出后删除临时目录里的自己
pub fn schedule_self_delete() {
    if !is_temp_copy() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // 只删自己复制出来的那份（文件名是 relaunch_from_temp_if_needed 起的）
    if !exe
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("tessel-uninstall-"))
    {
        return;
    }
    let script = format!("ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"", exe.display());
    let mut cmd = std::process::Command::new("cmd.exe");
    // cmd.exe 不认 Rust 默认的 \" 转义，命令行要原样传
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(format!("/C {script}"));
    }
    let _ = hidden(&mut cmd).spawn();
}

// 开机自启：和 Tessel 自己（设置里的开关）写同一个注册表值、同样的命令，两边互相认得
fn autostart_command(dir: &Path) -> String {
    format!("\"{}\" --background", app_exe(dir).display())
}

/// 是否已经登记了开机自启（更新时沿用用户原来的选择）
pub fn autostart_registered() -> bool {
    reg_get(RUN_KEY, RUN_VALUE).is_some()
}

pub fn set_autostart(dir: &Path, enabled: bool) -> Result<(), String> {
    // 模拟模式、测试实例都不改：这个注册表值只有一个，会影响正式安装的 Tessel
    if dry_run().is_some() || std::env::var_os("TESSEL_DATA_DIR").is_some() {
        dry_log(&format!("开机自启 = {enabled}"));
        return Ok(());
    }
    if enabled {
        reg_set(RUN_KEY, RUN_VALUE, &autostart_command(dir))
    } else {
        reg_delete(RUN_KEY, RUN_VALUE)
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn reg_get(key: &str, value: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::PCWSTR;
    let (key, value) = (wide(key), wide(value));
    let mut buf = [0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if err.is_err() {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len])).filter(|s| !s.is_empty())
}

#[cfg(windows)]
fn reg_set(key: &str, value: &str, data: &str) -> Result<(), String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    use windows::core::PCWSTR;
    let (key, value, data) = (wide(key), wide(value), wide(data));
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr() as *const _),
            (data.len() * 2) as u32,
        )
    }
    .ok()
    .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn reg_delete(key: &str, value: &str) -> Result<(), String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    use windows::core::PCWSTR;
    let (key, value) = (wide(key), wide(value));
    let err = unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
        )
    };
    // 本来就没有登记，也算成功
    if err == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    err.ok().map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn reg_get(_: &str, _: &str) -> Option<String> {
    None
}
#[cfg(not(windows))]
fn reg_set(_: &str, _: &str, _: &str) -> Result<(), String> {
    Ok(())
}
#[cfg(not(windows))]
fn reg_delete(_: &str, _: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_into_own_subfolder() {
        assert_eq!(
            install_target(Path::new(r"D:\Apps")),
            PathBuf::from(r"D:\Apps\Tessel")
        );
        assert_eq!(
            install_target(Path::new(r"D:\Apps\Tessel")),
            PathBuf::from(r"D:\Apps\Tessel")
        );
        assert_eq!(
            install_target(Path::new(r"D:\Apps\tessel.app")),
            PathBuf::from(r"D:\Apps\tessel.app")
        );
    }

    #[test]
    fn rejects_drive_root_and_data_dir() {
        assert!(check_target(Path::new(r"D:\")).is_err());
        assert!(check_target(&data_dir()).is_err());
        assert!(check_target(&data_dir().join("sub")).is_err());
        // 数据目录的上级（%LOCALAPPDATA%）本身也不行：会包含数据目录
        assert!(check_target(&local_app_data()).is_err());
    }

    #[test]
    fn leftover_uninstaller_does_not_block_reinstall() {
        let dir = std::env::temp_dir().join(format!("tessel-target-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("other.txt"), b"x").unwrap();
        assert!(check_target(&dir).is_err(), "有其他文件的文件夹要拒绝");
        std::fs::remove_file(dir.join("other.txt")).unwrap();
        std::fs::write(dir.join(UNINSTALLER), b"x").unwrap();
        assert!(check_target(&dir).is_ok(), "只剩上次的卸载程序时可以装");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
