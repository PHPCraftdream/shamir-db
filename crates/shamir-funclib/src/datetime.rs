//! `/datetime` scalar category — temporal functions over a canonical
//! **epoch-millis UTC `i64`** timestamp.
//!
//! Functions registered (plain names, no folder prefix):
//! `now to_epoch_s to_epoch_ms from_epoch_s from_epoch_ms parse_rfc3339
//!  format_rfc3339 year month day hour minute second weekday is_weekend
//!  add_secs add_days diff_secs start_of_day start_of_week start_of_month
//!  truncate age`.
//!
//! Conventions (mirroring `math.rs`):
//! - The canonical timestamp is `i64` epoch-millis UTC. Component extractors
//!   (`year`, `month`, …) and `weekday` return `Int`; `is_weekend` returns a
//!   `Bool`; `format_rfc3339` returns a `Str`; everything else returns the
//!   canonical `Int` epoch-millis.
//! - `now` and `age` read the wall clock and are therefore **non-deterministic
//!   and impure** (`pure:false, deterministic:false`); every other function is
//!   `pure + deterministic` and may back a functional index.
//! - Errors are machine codes only: `"out_of_range"` for un-representable
//!   timestamps, `"parse"` for an unparseable RFC-3339 string, `"bad_unit"` for
//!   an unknown `truncate` unit.

use crate::registry::{
    arg_i64, arg_str, v_bool, v_int, v_str, FnEntry, ScalarError, ScalarRegistry,
};
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc, Weekday};

/// Convert epoch-millis UTC into a `DateTime<Utc>`, or `"out_of_range"`.
fn to_dt(ms: i64) -> Result<DateTime<Utc>, ScalarError> {
    match Utc.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(dt) => Ok(dt),
        _ => Err(ScalarError::new("out_of_range")),
    }
}

/// First epoch-millis argument decoded into a `DateTime<Utc>`.
fn dt_arg(
    a: &[shamir_types::types::value::InnerValue],
    i: usize,
) -> Result<DateTime<Utc>, ScalarError> {
    to_dt(arg_i64(a, i)?)
}

/// Register the `/datetime` functions.
pub fn register(reg: &mut ScalarRegistry) {
    // ---- non-deterministic / impure: wall-clock readers --------------------
    reg.register(
        "now",
        FnEntry {
            f: std::sync::Arc::new(|_a: &[_]| Ok(v_int(Utc::now().timestamp_millis()))),
            min_args: 0,
            max_args: Some(0),
            pure: false,
            deterministic: false,
        },
    );
    reg.register(
        "age",
        FnEntry {
            f: std::sync::Arc::new(|a: &[_]| {
                let then = arg_i64(a, 0)?;
                Ok(v_int((Utc::now().timestamp_millis() - then) / 1000))
            }),
            min_args: 1,
            max_args: Some(1),
            pure: false,
            deterministic: false,
        },
    );

    // ---- epoch conversions -------------------------------------------------
    reg.register(
        "to_epoch_s",
        FnEntry::pure(|a| Ok(v_int(div_floor(arg_i64(a, 0)?, 1000))), 1, Some(1)),
    );
    reg.register(
        "to_epoch_ms",
        FnEntry::pure(|a| Ok(v_int(arg_i64(a, 0)?)), 1, Some(1)),
    );
    reg.register(
        "from_epoch_s",
        FnEntry::pure(
            |a| {
                arg_i64(a, 0)?
                    .checked_mul(1000)
                    .map(v_int)
                    .ok_or_else(|| ScalarError::new("out_of_range"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "from_epoch_ms",
        FnEntry::pure(|a| Ok(v_int(arg_i64(a, 0)?)), 1, Some(1)),
    );

    // ---- RFC-3339 parse / format ------------------------------------------
    reg.register(
        "parse_rfc3339",
        FnEntry::pure(
            |a| {
                let s = arg_str(a, 0)?;
                let dt = DateTime::parse_from_rfc3339(s)
                    .map_err(|_| ScalarError::new("parse"))?
                    .with_timezone(&Utc);
                Ok(v_int(dt.timestamp_millis()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "format_rfc3339",
        FnEntry::pure(
            |a| {
                let dt = dt_arg(a, 0)?;
                Ok(v_str(dt.to_rfc3339()))
            },
            1,
            Some(1),
        ),
    );

    // ---- component extractors ---------------------------------------------
    reg.register(
        "year",
        FnEntry::pure(|a| Ok(v_int(dt_arg(a, 0)?.year() as i64)), 1, Some(1)),
    );
    reg.register(
        "month",
        FnEntry::pure(|a| Ok(v_int(dt_arg(a, 0)?.month() as i64)), 1, Some(1)),
    );
    reg.register(
        "day",
        FnEntry::pure(|a| Ok(v_int(dt_arg(a, 0)?.day() as i64)), 1, Some(1)),
    );
    reg.register(
        "hour",
        FnEntry::pure(|a| Ok(v_int(dt_arg(a, 0)?.hour() as i64)), 1, Some(1)),
    );
    reg.register(
        "minute",
        FnEntry::pure(|a| Ok(v_int(dt_arg(a, 0)?.minute() as i64)), 1, Some(1)),
    );
    reg.register(
        "second",
        FnEntry::pure(|a| Ok(v_int(dt_arg(a, 0)?.second() as i64)), 1, Some(1)),
    );
    // weekday: 0 = Monday .. 6 = Sunday.
    reg.register(
        "weekday",
        FnEntry::pure(
            |a| Ok(v_int(dt_arg(a, 0)?.weekday().num_days_from_monday() as i64)),
            1,
            Some(1),
        ),
    );
    reg.register(
        "is_weekend",
        FnEntry::pure(
            |a| {
                let wd = dt_arg(a, 0)?.weekday();
                Ok(v_bool(matches!(wd, Weekday::Sat | Weekday::Sun)))
            },
            1,
            Some(1),
        ),
    );

    // ---- arithmetic --------------------------------------------------------
    reg.register(
        "add_secs",
        FnEntry::pure(
            |a| {
                let dt = dt_arg(a, 0)?;
                let n = arg_i64(a, 1)?;
                let delta =
                    Duration::try_seconds(n).ok_or_else(|| ScalarError::new("out_of_range"))?;
                let out = dt
                    .checked_add_signed(delta)
                    .ok_or_else(|| ScalarError::new("out_of_range"))?;
                Ok(v_int(out.timestamp_millis()))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "add_days",
        FnEntry::pure(
            |a| {
                let dt = dt_arg(a, 0)?;
                let n = arg_i64(a, 1)?;
                let delta =
                    Duration::try_days(n).ok_or_else(|| ScalarError::new("out_of_range"))?;
                let out = dt
                    .checked_add_signed(delta)
                    .ok_or_else(|| ScalarError::new("out_of_range"))?;
                Ok(v_int(out.timestamp_millis()))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "diff_secs",
        FnEntry::pure(
            |a| {
                let x = arg_i64(a, 0)?;
                let y = arg_i64(a, 1)?;
                Ok(v_int(div_floor(x - y, 1000)))
            },
            2,
            Some(2),
        ),
    );

    // ---- truncation / period starts ---------------------------------------
    reg.register(
        "start_of_day",
        FnEntry::pure(|a| Ok(v_int(start_of_day(dt_arg(a, 0)?)?)), 1, Some(1)),
    );
    reg.register(
        "start_of_week",
        FnEntry::pure(|a| Ok(v_int(start_of_week(dt_arg(a, 0)?)?)), 1, Some(1)),
    );
    reg.register(
        "start_of_month",
        FnEntry::pure(|a| Ok(v_int(start_of_month(dt_arg(a, 0)?)?)), 1, Some(1)),
    );
    reg.register(
        "truncate",
        FnEntry::pure(
            |a| {
                let dt = dt_arg(a, 0)?;
                let unit = arg_str(a, 1)?;
                let ms = match unit {
                    "second" => dt.timestamp_millis() - (dt.timestamp_millis().rem_euclid(1000)),
                    "minute" => start_of_minute(dt)?,
                    "hour" => start_of_hour(dt)?,
                    "day" => start_of_day(dt)?,
                    "week" => start_of_week(dt)?,
                    "month" => start_of_month(dt)?,
                    "year" => start_of_year(dt)?,
                    _ => return Err(ScalarError::new("bad_unit")),
                };
                Ok(v_int(ms))
            },
            2,
            Some(2),
        ),
    );
}

/// Floor division that rounds toward negative infinity (matches calendar
/// intuition for negative epoch values, unlike truncating `/`).
fn div_floor(n: i64, d: i64) -> i64 {
    n.div_euclid(d)
}

fn start_of_minute(dt: DateTime<Utc>) -> Result<i64, ScalarError> {
    let t = dt
        .with_second(0)
        .and_then(|d| d.with_nanosecond(0))
        .ok_or_else(|| ScalarError::new("out_of_range"))?;
    Ok(t.timestamp_millis())
}

fn start_of_hour(dt: DateTime<Utc>) -> Result<i64, ScalarError> {
    let t = dt
        .with_minute(0)
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .ok_or_else(|| ScalarError::new("out_of_range"))?;
    Ok(t.timestamp_millis())
}

fn start_of_day(dt: DateTime<Utc>) -> Result<i64, ScalarError> {
    let t = dt
        .with_hour(0)
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .ok_or_else(|| ScalarError::new("out_of_range"))?;
    Ok(t.timestamp_millis())
}

fn start_of_week(dt: DateTime<Utc>) -> Result<i64, ScalarError> {
    let days_back = dt.weekday().num_days_from_monday() as i64;
    let monday = dt
        .checked_sub_signed(Duration::days(days_back))
        .ok_or_else(|| ScalarError::new("out_of_range"))?;
    start_of_day(monday)
}

fn start_of_month(dt: DateTime<Utc>) -> Result<i64, ScalarError> {
    let t = dt
        .with_day(1)
        .and_then(|d| d.with_hour(0))
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .ok_or_else(|| ScalarError::new("out_of_range"))?;
    Ok(t.timestamp_millis())
}

fn start_of_year(dt: DateTime<Utc>) -> Result<i64, ScalarError> {
    let t = dt
        .with_month(1)
        .and_then(|d| d.with_day(1))
        .and_then(|d| d.with_hour(0))
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .ok_or_else(|| ScalarError::new("out_of_range"))?;
    Ok(t.timestamp_millis())
}

#[cfg(test)]
mod tests;
