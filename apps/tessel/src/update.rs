//! 检查更新：查询 GitHub 上最新的正式版本，有新版本时提示用户去下载。
//!
//! 只发一个 GET 请求读取公开的版本信息（不带任何个人数据），可以在设置里关闭。
//! 自动下载安装（Velopack 的 UpdateManager）等 velopack 1.3 出正式版后再接入：
//! 1.2.x 依赖的 zip 有解压漏洞，见 docs/toolchain.md。

use std::time::Duration;

pub const REPO: &str = "FarL1/tessel";
pub const RELEASES_PAGE: &str = "https://github.com/FarL1/tessel/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// 不带 v 前缀，例如 "0.2.1"
    pub version: String,
    /// 发布页面
    pub url: String,
}

/// 查询最新的正式版本；比当前运行的版本新时返回它
pub fn check() -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = match agent
        .get(&url)
        .header("User-Agent", concat!("Tessel/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(response) => response,
        // 还没有发布过正式版本
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let tag = json["tag_name"].as_str().unwrap_or_default();
    let version = tag.trim_start_matches('v').to_string();
    if !is_newer(&version, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let url = json["html_url"]
        .as_str()
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    Ok(Some(Release { version, url }))
}

/// "0.2.1" 是否比 "0.2.0" 新（只比较数字部分，忽略 -beta 之类的后缀）
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Option<(u64, u64, u64)> {
        let core = v.split(['-', '+']).next()?;
        let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
        Some((
            it.next()??,
            it.next().flatten().unwrap_or(0),
            it.next().flatten().unwrap_or(0),
        ))
    }
    match (parts(candidate), parts(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("garbage", "0.2.0"));
        assert!(is_newer("0.3", "0.2.5"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
    }
}
