use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use shamir_private_dir::verify_private_dir;

use super::ConfigError;

const SECONDS_PER_DAY: u64 = 86_400;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupScheduleConfig {
    pub destination: PathBuf,
    pub interval: String,
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default)]
    pub run_on_start: bool,
    #[serde(default)]
    pub retention: BackupRetentionConfig,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupRetentionConfig {
    #[serde(default = "default_keep_last")]
    pub keep_last: usize,
    #[serde(default)]
    pub max_age_days: Option<u32>,
}

impl Default for BackupRetentionConfig {
    fn default() -> Self {
        Self {
            keep_last: default_keep_last(),
            max_age_days: None,
        }
    }
}

fn default_keep_last() -> usize {
    7
}

#[derive(Debug, Clone)]
pub struct ResolvedSchedule {
    pub destination: PathBuf,
    pub interval: Duration,
    pub window: Option<DailyWindow>,
    pub run_on_start: bool,
    pub retention: BackupRetentionConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyWindow {
    start: u32,
    end: u32,
}

impl DailyWindow {
    pub fn contains(&self, minute: u32) -> bool {
        if minute >= 24 * 60 {
            return false;
        }
        if self.start < self.end {
            (self.start..self.end).contains(&minute)
        } else {
            minute >= self.start || minute < self.end
        }
    }

    fn parse(input: &str) -> Result<Self, ConfigError> {
        let [h1, h2, b':', m1, m2, b'-', h3, h4, b':', m3, m4] = *input.as_bytes() else {
            return Err(invalid_window());
        };
        let start = local_minute([h1, h2, m1, m2]).ok_or_else(invalid_window)?;
        let end = local_minute([h3, h4, m3, m4]).ok_or_else(invalid_window)?;
        if start == end {
            return Err(invalid_window());
        }
        Ok(Self { start, end })
    }
}

fn local_minute(digits: [u8; 4]) -> Option<u32> {
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let [h1, h2, m1, m2] = digits.map(|digit| u32::from(digit - b'0'));
    let hour = 10 * h1 + h2;
    let minute = 10 * m1 + m2;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn invalid_window() -> ConfigError {
    ConfigError::Validation("backup.window must be HH:MM-HH:MM with distinct endpoints".into())
}

impl BackupScheduleConfig {
    pub(super) fn validate(&self, data_dir: &Path) -> Result<(), ConfigError> {
        self.validated(data_dir).map(|_| ())
    }

    pub fn resolve(&self, data_dir: &Path) -> Result<ResolvedSchedule, ConfigError> {
        let mut schedule = self.validated(data_dir)?;
        let destination = self.destination.canonicalize().map_err(|error| {
            ConfigError::Validation(format!(
                "backup.destination cannot be canonicalized: {error}"
            ))
        })?;
        if destination.file_name().is_none() {
            return Err(invalid_destination());
        }
        let source = data_dir.canonicalize().map_err(|error| {
            ConfigError::Validation(format!("backup data_dir cannot be canonicalized: {error}"))
        })?;
        check_disjoint(&destination, &source)?;
        schedule.destination = destination;
        Ok(schedule)
    }

    fn validated(&self, data_dir: &Path) -> Result<ResolvedSchedule, ConfigError> {
        let interval = parse_interval(&self.interval)?;
        let window = self.window.as_deref().map(DailyWindow::parse).transpose()?;
        if self.retention.keep_last == 0 {
            return Err(ConfigError::Validation(
                "backup.retention.keep_last must be at least 1".into(),
            ));
        }
        // All u32 day counts fit in u64 seconds, including u32::MAX.
        if self.retention.max_age_days == Some(0) {
            return Err(ConfigError::Validation(
                "backup.retention.max_age_days must be at least 1".into(),
            ));
        }
        let destination = &self.destination;
        if !destination.is_absolute()
            || destination.file_name().is_none()
            || has_parent_component(destination)
        {
            return Err(invalid_destination());
        }
        if data_dir.is_absolute() && !has_parent_component(data_dir) {
            check_disjoint(destination, data_dir)?;
        }
        // A trailing separator or '/.' can make symlink_metadata follow the final link.
        let normalized: PathBuf = destination.components().collect();
        verify_private_dir(&normalized)
            .and_then(|()| {
                if destination.as_os_str() == normalized.as_os_str() {
                    Ok(())
                } else {
                    verify_private_dir(destination)
                }
            })
            .map_err(|error| {
                ConfigError::Validation(format!("backup.destination must be private: {error}"))
            })?;
        Ok(ResolvedSchedule {
            destination: destination.clone(),
            interval,
            window,
            run_on_start: self.run_on_start,
            retention: self.retention,
        })
    }
}

fn invalid_destination() -> ConfigError {
    ConfigError::Validation(
        "backup.destination must be an absolute non-root path without '..' components".into(),
    )
}

fn has_parent_component(path: &Path) -> bool {
    path.components().any(|part| part == Component::ParentDir)
}

fn check_disjoint(destination: &Path, source: &Path) -> Result<(), ConfigError> {
    if destination.starts_with(source) || source.starts_with(destination) {
        return Err(ConfigError::Validation(
            "backup.destination must be disjoint from data_dir".into(),
        ));
    }
    Ok(())
}

fn interval_overflow() -> ConfigError {
    ConfigError::Validation("backup.interval is too large".into())
}

fn parse_interval(input: &str) -> Result<Duration, ConfigError> {
    let syntax = || {
        ConfigError::Validation(
            "backup.interval must be a whole ASCII number followed by s, m, h or d".into(),
        )
    };
    let (unit, digits) = input.as_bytes().split_last().ok_or_else(syntax)?;
    let multiplier = match unit {
        b's' => 1,
        b'm' => 60,
        b'h' => 3_600,
        b'd' => SECONDS_PER_DAY,
        _ => return Err(syntax()),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(syntax());
    }
    let count: u64 = input[..digits.len()]
        .parse()
        .map_err(|_| interval_overflow())?;
    let seconds = count
        .checked_mul(multiplier)
        .ok_or_else(interval_overflow)?;
    if seconds < 60 {
        return Err(ConfigError::Validation(
            "backup.interval must be at least 60 seconds".into(),
        ));
    }
    let interval = Duration::from_secs(seconds);
    Instant::now()
        .checked_add(interval)
        .ok_or_else(interval_overflow)?;
    Ok(interval)
}
