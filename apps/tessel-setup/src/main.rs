//! Tessel 安装 / 卸载程序。
//!
//! - 下载页上的 `Tessel-Setup.exe`：安装界面。Velopack 自带的安装程序只有"一键安装"，界面不能定制
//!   （已安装时还会弹一个英文的系统对话框），所以我们自己画：欢迎页（可以选安装位置）→ 安装中 →
//!   完成（由用户选择是否启动、是否开机自启）。真正的安装交给内置的 Velopack Setup.exe，以静默模式完成。
//! - 安装目录里的 `Uninstall Tessel.exe`（同一个程序，不带安装包）以 `--uninstall` 启动：卸载界面，
//!   "设置 → 应用"里的卸载入口指向它。
//!
//! 安装目录结构、更新方式都和直接用 Velopack 安装一样。

// release 构建用 GUI 子系统：双击时不弹出控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod payload;
mod system;
mod ui;
mod uninstall;

use tessel_core::Config;
use tessel_core::i18n;

fn main() {
    let uninstalling = std::env::args().any(|a| a == "--uninstall");
    // 卸载会删除整个安装目录：先把自己复制到临时目录再运行（模拟模式下不需要）
    if uninstalling && system::dry_run().is_none() && system::relaunch_from_temp_if_needed() {
        return;
    }

    // 语言和热键：已经装过的话沿用用户的设置（只读，不创建任何文件）
    let config = std::fs::read_to_string(Config::path(&system::data_dir()))
        .ok()
        .and_then(|text| Config::parse(&text).ok());
    let language = config.as_ref().map(|c| c.language).unwrap_or_default();
    let english = match std::env::var("TESSEL_SETUP_LANG").as_deref() {
        Ok("en") => true,
        Ok("zh") => false,
        _ => language.resolve(system::system_is_chinese()),
    };
    i18n::set_english(english);
    let hotkey = config
        .map(|c| c.hotkey)
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "Alt+V".into());

    gpui_kit::application()
        .with_assets(ui::SetupAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            if uninstalling {
                uninstall::open(cx);
            } else {
                install::open(hotkey, cx);
            }
        });
}
