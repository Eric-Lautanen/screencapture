//! Timestamps for file names: UTC, `std` math only ("20261019-153045").
//!
//! One stamp per file; local-time conversion arrives with the settings
//! surface for it. The calendar math is Howard Hinnant's civil algorithm.

use std::path::{Path, PathBuf};

/// Fresh stamped file path inside `media_dir`; appends `-2`, `-3`, ... on
/// same-second clashes so back-to-back captures never overwrite each other.
#[must_use]
pub fn stamped_path(media_dir: &Path, ext: &str) -> PathBuf {
    stamped_path_for(media_dir, &stamp(), ext)
}

fn stamped_path_for(media_dir: &Path, stamp: &str, ext: &str) -> PathBuf {
    let stem = format!("screencapture-{stamp}");
    let mut path = media_dir.join(format!("{stem}.{ext}"));
    let mut counter = 1;
    while path.exists() {
        counter += 1;
        path = media_dir.join(format!("{stem}-{counter}.{ext}"));
    }
    path
}

/// Stamp of "now", UTC, in `YYYYMMDD-HHMMSS` form.
#[must_use]
pub fn stamp() -> String {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    from_epoch(since_epoch.as_secs())
}

fn from_epoch(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = time_of_day(seconds_of_day);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn time_of_day(seconds: u64) -> (u64, u64, u64) {
    (seconds / 3600, (seconds % 3600) / 60, seconds % 60)
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(days: u64) -> (i64, u32, u32) {
    let z = i64::try_from(days).unwrap_or(i64::MAX / 2) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, from_epoch, stamped_path_for};

    #[test]
    fn same_second_captures_never_overwrite_each_other() {
        let dir =
            std::env::temp_dir().join(format!("screencapture-stamp-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let first = stamped_path_for(&dir, "20261020-120000", "mp4");
        assert_eq!(first, dir.join("screencapture-20261020-120000.mp4"));
        std::fs::write(&first, b"clip").expect("write first");
        let second = stamped_path_for(&dir, "20261020-120000", "mp4");
        assert_eq!(second, dir.join("screencapture-20261020-120000-2.mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn epoch_day_zero_is_unix_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn year_rollover_counts_leap_years() {
        assert_eq!(civil_from_days(365), (1971, 1, 1));
        // Days from 1970-01-01 to 2025-01-01: 55 years with 14 leap days.
        let days_to_2025 = 55 * 365 + 14;
        assert_eq!(civil_from_days(days_to_2025), (2025, 1, 1));
    }

    #[test]
    fn formats_stamps() {
        assert_eq!(from_epoch(0), "19700101-000000");
        assert_eq!(from_epoch(86_400 + 59), "19700102-000059");
        assert_eq!(from_epoch(86_400 + 6 * 3600 + 900), "19700102-061500");
    }
}
