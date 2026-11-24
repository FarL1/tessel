//! 相对时间："刚刚"、"5 分钟前"、"昨天"……

use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::tr;
use crate::trf;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// 按东八区的自然日计算"昨天"（M1 先固定时区，之后改为读取系统时区）
const TZ_OFFSET_MS: i64 = 8 * 3600 * 1000;
const DAY_MS: i64 = 86_400_000;

pub fn relative(ts_ms: i64, now_ms: i64) -> String {
    let diff = (now_ms - ts_ms).max(0);
    let minutes = diff / 60_000;
    if minutes < 1 {
        return tr("刚刚", "just now").into();
    }
    if minutes < 60 {
        return trf!("{minutes} 分钟前", "{minutes} min ago");
    }
    let day_now = (now_ms + TZ_OFFSET_MS).div_euclid(DAY_MS);
    let day_ts = (ts_ms + TZ_OFFSET_MS).div_euclid(DAY_MS);
    match day_now - day_ts {
        0 => trf!("{} 小时前", "{} h ago", minutes / 60),
        1 => tr("昨天", "yesterday").into(),
        d @ 2..=6 => trf!("{d} 天前", "{d} days ago"),
        _ => {
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let (_, m, d) = civil_from_days(day_ts);
            let month = MONTHS[m as usize - 1];
            trf!("{m} 月 {d} 日", "{month} {d}")
        }
    }
}

/// Howard Hinnant 的 days → (年, 月, 日) 算法
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        let now = 1_795_000_000_000;
        assert_eq!(relative(now - 10_000, now), "刚刚");
        assert_eq!(relative(now - 5 * 60_000, now), "5 分钟前");
        assert!(relative(now - 10 * DAY_MS, now).contains('月'));
    }

    #[test]
    fn civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_416), (2025, 11, 24));
    }
}
