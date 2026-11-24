//! 界面语言：简体中文 / English。
//!
//! 只有两种语言，所以不用翻译文件，而是把两种写法放在同一处：`tr("复制", "Copy")`，
//! 带参数的用 [`trf!`]。当前语言是进程级的全局设置，切换后各窗口重绘即可生效。

#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;

/// 配置文件里的 `language`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LanguagePref {
    /// 跟随 Windows 显示语言（中文系统用中文，其他用英文）
    #[default]
    System,
    Zh,
    En,
}

impl LanguagePref {
    pub const ALL: [LanguagePref; 3] = [LanguagePref::System, LanguagePref::Zh, LanguagePref::En];

    pub fn as_str(self) -> &'static str {
        match self {
            LanguagePref::System => "system",
            LanguagePref::Zh => "zh",
            LanguagePref::En => "en",
        }
    }

    /// 设置里显示的名字。语言名用它自己的写法，不随界面语言变化
    pub fn label(self) -> &'static str {
        match self {
            LanguagePref::System => tr("跟随系统", "System"),
            LanguagePref::Zh => "简体中文",
            LanguagePref::En => "English",
        }
    }

    /// 解析为实际使用的语言。`system_is_chinese`：系统显示语言是不是中文
    pub fn resolve(self, system_is_chinese: bool) -> bool {
        match self {
            LanguagePref::System => !system_is_chinese,
            LanguagePref::Zh => false,
            LanguagePref::En => true,
        }
    }
}

#[cfg(not(test))]
static ENGLISH: AtomicBool = AtomicBool::new(false);

// 测试并行运行：每个测试线程各有一份，切换语言的测试不会影响其他测试
#[cfg(test)]
thread_local! {
    static ENGLISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 设置当前界面语言（`true` = English）
pub fn set_english(english: bool) {
    #[cfg(not(test))]
    ENGLISH.store(english, Ordering::Relaxed);
    #[cfg(test)]
    ENGLISH.with(|e| e.set(english));
}

pub fn is_en() -> bool {
    #[cfg(not(test))]
    return ENGLISH.load(Ordering::Relaxed);
    #[cfg(test)]
    ENGLISH.with(|e| e.get())
}

/// 按当前语言选一种写法
pub fn tr(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { zh }
}

/// 带参数的 [`tr`]：`trf!("已删除 {n} 条", "Deleted {n}")`，两边都按 `format!` 处理
#[macro_export]
macro_rules! trf {
    ($zh:literal, $en:literal $(, $args:expr)* $(,)?) => {
        if $crate::i18n::is_en() {
            format!($en $(, $args)*)
        } else {
            format!($zh $(, $args)*)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch() {
        set_english(false);
        assert_eq!(tr("复制", "Copy"), "复制");
        let n = 3;
        assert_eq!(trf!("{n} 条", "{n} items"), "3 条");
        set_english(true);
        assert_eq!(tr("复制", "Copy"), "Copy");
        assert_eq!(trf!("{n} 条", "{n} items"), "3 items");
        set_english(false);
        assert!(LanguagePref::System.resolve(false));
        assert!(!LanguagePref::System.resolve(true));
    }
}
