//! 用户配置：`<数据目录>/config.toml`。
//!
//! "配置即文件"：设置界面（以后）和用户手动编辑读写的是同一个文件，方便备份和同步。
//! 文件被修改后应用会自动重新加载；解析失败时保留旧配置并提示错误。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::i18n::{LanguagePref, tr};
use crate::trf;

pub const FILE_NAME: &str = "config.toml";

/// 首次运行时写出的默认配置（带注释，方便用户直接编辑）
pub const DEFAULT_FILE: &str = r#"# Tessel · 拼贴 配置文件
# 保存后自动生效，不需要重启。

# 唤起弹窗的全局热键。示例："Alt+V"、"Ctrl+Shift+V"、"Win+Alt+V"
hotkey = "Alt+V"

# 粘贴队列：在弹窗里按 Ctrl+Q 把记录排进队列，之后每按一次这个热键，就粘贴队列里的下一条。
# 留空（""）表示不使用。
queue_hotkey = "Alt+Shift+V"

# 最多保留多少条历史（收藏的不计入、永不自动删除）
max_items = 5000

# 选中后的行为：true = 直接粘贴到之前的窗口；false = 只复制到剪贴板
auto_paste = true

# 每次复制成功时，在屏幕底部显示一个小提示
copy_toast = true

# 每天检查一次 GitHub 上有没有新版本（只读取公开的版本号，不发送任何个人数据）
check_updates = true

# 外观："system"（跟随系统）、"light"、"dark"
theme = "system"

# 界面语言："system"（跟随 Windows 显示语言）、"zh"（简体中文）、"en"（English）
language = "system"

# 不记录这些应用复制的内容（按 exe 文件名匹配，不区分大小写）
# 默认包含常见的密码管理器；它们大多已经设置了系统隐私标记，这里是双保险。
ignored_apps = [
    "KeePass.exe",
    "KeePassXC.exe",
    "1Password.exe",
    "Bitwarden.exe",
    "Dashlane.exe",
    "LastPass.exe",
    "Enpass.exe",
    "NordPass.exe",
    "Keeper.exe",
]
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::System, ThemePref::Light, ThemePref::Dark];

    pub fn as_str(self) -> &'static str {
        match self {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::System => tr("跟随系统", "System"),
            ThemePref::Light => tr("浅色", "Light"),
            ThemePref::Dark => tr("深色", "Dark"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub hotkey: String,
    pub queue_hotkey: String,
    pub max_items: usize,
    pub auto_paste: bool,
    pub copy_toast: bool,
    pub check_updates: bool,
    pub theme: ThemePref,
    pub language: LanguagePref,
    pub ignored_apps: Vec<String>,
}

impl Default for Config {
    /// 必须与 DEFAULT_FILE 保持一致（有测试保证）
    fn default() -> Self {
        Self {
            hotkey: "Alt+V".into(),
            queue_hotkey: "Alt+Shift+V".into(),
            max_items: 5000,
            auto_paste: true,
            copy_toast: true,
            check_updates: true,
            theme: ThemePref::System,
            language: LanguagePref::System,
            ignored_apps: [
                "KeePass.exe",
                "KeePassXC.exe",
                "1Password.exe",
                "Bitwarden.exe",
                "Dashlane.exe",
                "LastPass.exe",
                "Enpass.exe",
                "NordPass.exe",
                "Keeper.exe",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

impl Config {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// 读取配置；文件不存在时写出带注释的默认文件
    pub fn load_or_create(data_dir: &Path) -> Result<Config, String> {
        let path = Self::path(data_dir);
        if !path.exists() {
            std::fs::write(&path, DEFAULT_FILE).map_err(|e| {
                trf!(
                    "无法创建配置文件：{e}",
                    "Cannot create the config file: {e}"
                )
            })?;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| trf!("无法读取配置文件：{e}", "Cannot read the config file: {e}"))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let mut config: Config = toml::from_str(text).map_err(|e| {
            let line = e
                .span()
                .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
            match line {
                Some(line) => trf!(
                    "config.toml 第 {line} 行有误：{}",
                    "config.toml line {line}: {}",
                    e.message()
                ),
                None => trf!("config.toml 有误：{}", "config.toml: {}", e.message()),
            }
        })?;
        if config.max_items < 10 {
            return Err(tr("max_items 至少为 10", "max_items must be at least 10").into());
        }
        config.hotkey = config.hotkey.trim().to_string();
        config.queue_hotkey = config.queue_hotkey.trim().to_string();
        Ok(config)
    }

    /// 写回配置文件：只修改各项的值，保留文件里的注释和用户自己的排版。
    /// 先写临时文件再重命名，避免写到一半时被读到（或断电）导致文件损坏。
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        use toml_edit::{Array, DocumentMut, value};

        let path = Self::path(data_dir);
        // 文件不存在时从带注释的模板开始；文件存在但不是合法 TOML 时拒绝保存，
        // 绝不能用模板覆盖用户写了一半的配置
        let existing = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_FILE.into(),
            Err(e) => {
                return Err(trf!(
                    "无法读取配置文件：{e}",
                    "Cannot read the config file: {e}"
                ));
            }
        };
        let mut doc: DocumentMut = existing.parse().map_err(|_| {
            tr(
                "config.toml 有语法错误，请先修正后再保存",
                "config.toml has a syntax error; fix it before saving",
            )
            .to_string()
        })?;

        doc["hotkey"] = value(self.hotkey.as_str());
        doc["queue_hotkey"] = value(self.queue_hotkey.as_str());
        doc["copy_toast"] = value(self.copy_toast);
        doc["check_updates"] = value(self.check_updates);
        doc["max_items"] = value(self.max_items as i64);
        doc["auto_paste"] = value(self.auto_paste);
        doc["theme"] = value(self.theme.as_str());
        doc["language"] = value(self.language.as_str());
        let mut apps = Array::new();
        for app in &self.ignored_apps {
            apps.push(app.as_str());
        }
        for item in apps.iter_mut() {
            item.decor_mut().set_prefix("\n    ");
        }
        apps.set_trailing("\n");
        apps.set_trailing_comma(true);
        doc["ignored_apps"] = value(apps);

        let tmp = path.with_extension("toml.tmp");
        let write_err =
            |e: std::io::Error| trf!("无法写入配置文件：{e}", "Cannot write the config file: {e}");
        std::fs::write(&tmp, doc.to_string()).map_err(write_err)?;
        std::fs::rename(&tmp, &path).map_err(write_err)
    }

    /// `exe_path` 可以是完整路径或文件名
    pub fn is_ignored(&self, exe_path: &str) -> bool {
        let name = crate::model::file_name(exe_path);
        self.ignored_apps
            .iter()
            .any(|app| app.trim().eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let c = Config::default();
        assert_eq!(
            Config::parse(DEFAULT_FILE).unwrap(),
            c,
            "Default 与 DEFAULT_FILE 不一致"
        );
        assert_eq!(Config::parse("").unwrap(), c);
        assert_eq!(c.hotkey, "Alt+V");
        assert_eq!(c.max_items, 5000);
        assert!(c.auto_paste);
        assert!(c.copy_toast);
        assert_eq!(c.queue_hotkey, "Alt+Shift+V");
        assert_eq!(c.theme, ThemePref::System);
        assert!(c.is_ignored(r"C:\Program Files\KeePassXC\keepassxc.EXE"));
        assert!(!c.is_ignored("notepad.exe"));
    }

    #[test]
    fn save_keeps_comments_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("tessel-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(Config::path(&dir));
        let mut c = Config::load_or_create(&dir).unwrap();
        c.hotkey = "Ctrl+Shift+V".into();
        c.max_items = 20000;
        c.auto_paste = false;
        c.copy_toast = false;
        c.check_updates = false;
        c.queue_hotkey = String::new();
        c.theme = ThemePref::Dark;
        c.language = LanguagePref::En;
        c.ignored_apps = vec!["KeePassXC.exe".into(), "微信.exe".into()];
        c.save(&dir).unwrap();

        let text = std::fs::read_to_string(Config::path(&dir)).unwrap();
        assert!(text.contains("# 唤起弹窗的全局热键"), "注释应保留");
        assert_eq!(Config::parse(&text).unwrap(), c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_refuses_to_overwrite_broken_file() {
        let dir = std::env::temp_dir().join(format!("tessel-cfg-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let broken = "hotkey = \"Ctrl+Q\"\ntheme = [未闭合\n";
        std::fs::write(Config::path(&dir), broken).unwrap();
        assert!(Config::default().save(&dir).is_err());
        assert_eq!(std::fs::read_to_string(Config::path(&dir)).unwrap(), broken);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let c = Config::parse("theme = \"dark\"\n").unwrap();
        assert_eq!(c.theme, ThemePref::Dark);
        assert_eq!(c.max_items, 5000);
    }

    #[test]
    fn errors_are_reported() {
        let err = Config::parse(
            "theme = \"dark\"
max_items = \"many\"",
        )
        .unwrap_err();
        assert!(err.contains("第 2 行"), "{err}");
        assert!(Config::parse("max_items = 1").is_err());
        assert!(Config::parse("unknown_key = 1").is_err());
        assert!(Config::parse("theme = \"pink\"").is_err());
    }
}
