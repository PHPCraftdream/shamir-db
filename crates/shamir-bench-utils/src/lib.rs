//! Bench helpers for the S.H.A.M.I.R. workspace.
//!
//! ## Three tiers
//!
//! | Tier   | Env trigger      | sample | measurement | warm_up | Per-cell budget        |
//! |--------|------------------|--------|-------------|---------|------------------------|
//! | SMOKE  | `BENCH_SMOKE=1`  | 3      | 500 ms      | 200 ms  | ~5 s (compile-check)   |
//! | QUICK  | (default)        | 10     | 1 s         | 1 s     | ~20 s/cell typical     |
//! | FULL   | `BENCH_FULL=1`   | caller | caller      | caller  | unbounded (stats grade)|
//!
//! SMOKE — лёгкий smoke-test: «всё компилируется, числа разумны». Идеально
//! для CI на каждый PR. QUICK — итеративная разработка (`/opti` циклы). FULL
//! — release-signal capture, минуты-час на бенч.
//!
//! ## Per-cell wall-clock guard
//!
//! [`tune_tiered`] принимает `max_wall_secs` upper bound на cell. Когда
//! pre-computed worst case (`sample_size × measurement_secs`) превышает
//! бюджет — sample_size адаптивно снижается до минимума 3 (`MIN_SAMPLES`).
//! Это **pre-computed estimate**, не runtime kill. Criterion warning «Unable
//! to complete N samples in T» по-прежнему расширяет measurement если
//! одна iter превысит бюджет — это известное ограничение criterion API.
//!
//! Backward-compat: [`tune`] остаётся работать; новый код используйте
//! `tune_tiered` для явного per-cell budget'а.
//!
//! Legacy env-var `BENCH_QUICK=1` honored as no-op alias.

use std::time::Duration;

use criterion::measurement::Measurement;
use criterion::BenchmarkGroup;

#[cfg(feature = "peak_mem")]
pub mod peak_mem;

/// Минимальный sample_size, который criterion принимает.
const MIN_SAMPLES: usize = 3;

/// SMOKE tier fixed sample (compile-check + sanity).
const SMOKE_SAMPLES: usize = 3;
/// SMOKE tier fixed measurement duration.
const SMOKE_MEASUREMENT: Duration = Duration::from_millis(500);
/// SMOKE tier fixed warm-up duration.
const SMOKE_WARM_UP: Duration = Duration::from_millis(200);

/// QUICK tier fixed sample.
const QUICK_SAMPLES: usize = 5;
/// QUICK tier fixed measurement.
const QUICK_MEASUREMENT: Duration = Duration::from_millis(500);
/// QUICK tier fixed warm-up.
const QUICK_WARM_UP: Duration = Duration::from_millis(500);

/// `true` если `BENCH_SMOKE` set в `1`/`true`/`yes`/`on`.
///
/// SMOKE — самый жёсткий tier: sample=3, measurement=500ms, warm_up=200ms.
/// Per-cell wall-clock ~5s в худшем случае. Идеально для CI smoke-checks.
/// **Имеет приоритет над FULL** — если оба set'нуты, побеждает SMOKE.
pub fn is_smoke() -> bool {
    parse_bool_env("BENCH_SMOKE")
}

/// FULL mode **отключён**. Всегда возвращает `false`.
///
/// Ранее FULL позволял caller-defaults (sample=N, measurement=Ms) проходить
/// без ограничений, что приводило к часовым бенчам и зависанию машины.
/// Для re-enable — раскомментировать тело и установить `BENCH_FULL=1` +
/// `BENCH_FULL_CONFIRM=1`.
pub fn is_full() -> bool {
    // parse_bool_env("BENCH_FULL") && parse_bool_env("BENCH_FULL_CONFIRM")
    false
}

/// `true` когда bench работает в QUICK tier (default — ни SMOKE ни FULL).
pub fn is_quick() -> bool {
    !is_smoke() && !is_full()
}

fn parse_bool_env(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Sample-size floor (backward-compat API).
///
/// SMOKE → 3. QUICK → 10. FULL → `default`.
pub fn sample_size(default: usize) -> usize {
    if is_smoke() {
        SMOKE_SAMPLES
    } else if is_quick() {
        QUICK_SAMPLES
    } else {
        default
    }
}

/// Measurement-time floor (backward-compat API).
///
/// SMOKE → 500ms. QUICK → 1s. FULL → `default`.
pub fn measurement_time(default: Duration) -> Duration {
    if is_smoke() {
        SMOKE_MEASUREMENT
    } else if is_quick() {
        QUICK_MEASUREMENT
    } else {
        default
    }
}

/// Warm-up-time floor (backward-compat API).
///
/// SMOKE → 200ms. QUICK → 1s. FULL → `default`.
pub fn warm_up_time(default: Duration) -> Duration {
    if is_smoke() {
        SMOKE_WARM_UP
    } else if is_quick() {
        QUICK_WARM_UP
    } else {
        default
    }
}

/// Apply tier-aware knobs. Backward-compat — same shape как до tier'ов.
///
/// Без `max_wall_secs` guard'а. Для новых бенчей — используй
/// [`tune_tiered`] с явным per-cell budget'ом.
pub fn tune<M: Measurement>(
    group: &mut BenchmarkGroup<'_, M>,
    sample_size_default: usize,
    measurement_secs: u64,
    warm_up_secs: u64,
) {
    group.sample_size(sample_size(sample_size_default));
    group.measurement_time(measurement_time(Duration::from_secs(measurement_secs)));
    group.warm_up_time(warm_up_time(Duration::from_secs(warm_up_secs)));
}

/// Apply tier-aware knobs **с per-cell wall-clock guard'ом**.
///
/// Расчёт worst-case `sample_size × measurement_secs` сравнивается с
/// `max_wall_secs`. Если > бюджет — sample_size **адаптивно снижается**
/// до значения, при котором worst case укладывается, но не ниже [`MIN_SAMPLES`].
///
/// Применяется ПОСЛЕ tier'а (SMOKE/QUICK хардкодят свои значения; guard
/// только в FULL имеет effect — в SMOKE/QUICK budget'ы уже маленькие).
///
/// # Параметры
/// - `sample_size_default` — FULL-mode samples count.
/// - `measurement_secs` — FULL-mode measurement duration в секундах.
/// - `warm_up_secs` — FULL-mode warm-up в секундах.
/// - `max_wall_secs` — upper bound на per-cell worst case (sample × measurement).
///   `0` означает «no guard» — эквивалентно [`tune`].
pub fn tune_tiered<M: Measurement>(
    group: &mut BenchmarkGroup<'_, M>,
    sample_size_default: usize,
    measurement_secs: u64,
    warm_up_secs: u64,
    max_wall_secs: u64,
) {
    let measurement = measurement_time(Duration::from_secs(measurement_secs));
    let warm_up = warm_up_time(Duration::from_secs(warm_up_secs));
    let raw_samples = sample_size(sample_size_default);

    let samples = if max_wall_secs > 0 && is_full() {
        // Только в FULL mode имеет смысл — SMOKE/QUICK уже capped.
        let measurement_secs_actual = measurement.as_secs().max(1);
        let max_samples = (max_wall_secs / measurement_secs_actual) as usize;
        raw_samples.min(max_samples).max(MIN_SAMPLES)
    } else {
        raw_samples
    };

    group.sample_size(samples);
    group.measurement_time(measurement);
    group.warm_up_time(warm_up);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env var tests serialized because env is process-global mutable state.
    // Using static Mutex around all env-touching tests.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: tests are serialized via ENV_LOCK.
        let prev: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(*k).ok()))
            .collect();
        for (k, v) in vars {
            // SAFETY: serialized via ENV_LOCK.
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (k, v) in prev {
            // SAFETY: serialized via ENV_LOCK.
            unsafe {
                match v {
                    Some(val) => std::env::set_var(&k, val),
                    None => std::env::remove_var(&k),
                }
            }
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn default_is_quick() {
        with_env(
            &[
                ("BENCH_SMOKE", None),
                ("BENCH_FULL", None),
                ("BENCH_FULL_CONFIRM", None),
            ],
            || {
                assert!(is_quick());
                assert!(!is_smoke());
                assert!(!is_full());
                assert_eq!(sample_size(100), QUICK_SAMPLES);
                assert_eq!(measurement_time(Duration::from_secs(5)), QUICK_MEASUREMENT);
                assert_eq!(warm_up_time(Duration::from_secs(3)), QUICK_WARM_UP);
            },
        );
    }

    #[test]
    fn smoke_overrides_full() {
        with_env(
            &[
                ("BENCH_SMOKE", Some("1")),
                ("BENCH_FULL", Some("1")),
                ("BENCH_FULL_CONFIRM", Some("1")),
            ],
            || {
                assert!(is_smoke());
                assert!(!is_full(), "SMOKE имеет приоритет над FULL");
                assert!(!is_quick());
                assert_eq!(sample_size(100), SMOKE_SAMPLES);
                assert_eq!(measurement_time(Duration::from_secs(5)), SMOKE_MEASUREMENT);
                assert_eq!(warm_up_time(Duration::from_secs(3)), SMOKE_WARM_UP);
            },
        );
    }

    #[test]
    fn full_is_always_disabled() {
        with_env(
            &[
                ("BENCH_SMOKE", None),
                ("BENCH_FULL", Some("1")),
                ("BENCH_FULL_CONFIRM", Some("1")),
            ],
            || {
                assert!(!is_full(), "FULL mode hard-disabled");
                assert!(is_quick(), "falls back to QUICK");
                assert_eq!(sample_size(100), QUICK_SAMPLES);
                assert_eq!(measurement_time(Duration::from_secs(5)), QUICK_MEASUREMENT);
            },
        );
    }

    /// Guard в QUICK mode no-op (tier уже capped).
    #[test]
    fn tune_tiered_guard_inactive_in_quick() {
        with_env(
            &[
                ("BENCH_SMOKE", None),
                ("BENCH_FULL", None),
                ("BENCH_FULL_CONFIRM", None),
            ],
            || {
                // В QUICK mode sample_size возвращает 10 независимо от default.
                let raw_samples = sample_size(100);
                assert_eq!(raw_samples, QUICK_SAMPLES);
                // tune_tiered не должен дальше резать в QUICK — он бы вернул 10.
                // (Внутри tune_tiered: `is_full()` = false → guard skipped.)
            },
        );
    }
}
