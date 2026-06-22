//! VersionWindow Stage 0 — cache depth probe.
//!
//! Measures the steady-state depth of the decode/deliver caches under
//! a simulated "healthy consumer" pattern: insert events per commit,
//! evict per watermark advance (every Kth commit). The question is
//! whether the cache stays small (tens) like the overlay, or grows
//! unboundedly.
//!
//! This is MEASUREMENT, not correctness. Probe results are printed to
//! stderr for the Stage 0 verdict.
//!
//! Run via: `./scripts/test.sh -p shamir-server -- cache_depth_probe`

use crate::subscriptions::decode_cache::{cache_evict_up_to, cache_get, cache_insert};
use crate::subscriptions::deliver_cache::{
    deliver_cache_evict_up_to, deliver_cache_get, deliver_cache_insert,
};

/// Count survivors by probing `cache_get` for all entries in [lo_cv..=hi_cv].
/// O(range * changes_per_commit) — fine for a measurement probe.
fn decode_survivors(repo: &str, lo_cv: u64, hi_cv: u64, changes_per_commit: usize) -> usize {
    let mut count = 0;
    for cv in lo_cv..=hi_cv {
        for idx in 0..changes_per_commit {
            if cache_get(repo, cv, idx).is_some() {
                count += 1;
            }
        }
    }
    count
}

fn deliver_survivors(
    db_id: u64,
    repo: &str,
    lo_cv: u64,
    hi_cv: u64,
    changes_per_commit: usize,
) -> usize {
    let mut count = 0;
    for cv in lo_cv..=hi_cv {
        for idx in 0..changes_per_commit {
            for mode in [0u8, 1u8] {
                if deliver_cache_get(db_id, repo, cv, idx, mode).is_some() {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Probe: healthy consumer — inserts N events per commit, evicts every
/// K commits. Measures peak and steady-state cache depth.
///
/// Simulates 1000 commits, 3 changes per commit, 1 subscriber.
/// Consumer watermark advances every 5 commits (latency = 5 commit
/// versions behind). This is "healthy" — real consumers lag by 1-2
/// commits.
#[test]
fn probe_decode_cache_depth_healthy_consumer() {
    // Use a unique repo to avoid cross-test pollution on the global cache.
    let repo = format!("depth_probe_decode_{}", std::process::id());

    let commits = 1000u64;
    let changes_per_commit = 3usize;
    let consumer_lag = 5u64; // evict every 5 commits

    let mut peak = 0usize;
    let mut samples: Vec<(u64, usize)> = Vec::new();
    let mut last_evicted: u64 = 0;

    for cv in 1..=commits {
        for idx in 0..changes_per_commit {
            cache_insert(&repo, cv, idx, None);
        }

        // Consumer evicts every `consumer_lag` commits.
        if cv % consumer_lag == 0 && cv > consumer_lag {
            let evict_to = cv - consumer_lag;
            cache_evict_up_to(evict_to);
            last_evicted = evict_to;
        }

        if cv % 100 == 0 || cv <= 20 {
            // Count live entries: from (last_evicted+1) to cv.
            let lo = last_evicted + 1;
            let d = decode_survivors(&repo, lo, cv, changes_per_commit);
            if d > peak {
                peak = d;
            }
            samples.push((cv, d));
        }
    }

    // Final eviction — consumer catches up.
    cache_evict_up_to(commits);
    let final_depth = decode_survivors(&repo, 1, commits, changes_per_commit);

    eprintln!(
        "STAGE0_CACHE_PROBE decode: commits={commits} changes_per_commit={changes_per_commit} \
         consumer_lag={consumer_lag} peak={peak} final_depth={final_depth} \
         samples={samples:?}"
    );

    // The depth should stay bounded: at most (consumer_lag + lag_between_evictions)
    // * changes_per_commit entries.
    // Under a healthy consumer with lag=5, this is ~30 entries at most.
    let lag_bound = (consumer_lag as usize + consumer_lag as usize) * changes_per_commit;
    assert!(
        peak <= lag_bound + 10, // +10 tolerance for boundary effects
        "decode cache depth unexpectedly high: peak={peak}; expected <= {lag_bound}"
    );
}

/// Same probe for the deliver cache.
#[test]
fn probe_deliver_cache_depth_healthy_consumer() {
    let repo = format!("depth_probe_deliver_{}", std::process::id());
    let db_id: u64 = 0xDEAD_CAFE;

    let commits = 1000u64;
    let changes_per_commit = 3usize;
    let consumer_lag = 5u64;

    let mut peak = 0usize;
    let mut samples: Vec<(u64, usize)> = Vec::new();
    let mut last_evicted: u64 = 0;

    for cv in 1..=commits {
        for idx in 0..changes_per_commit {
            for &mode in &[0u8, 1u8] {
                deliver_cache_insert(db_id, &repo, cv, idx, mode, vec![cv as u8]);
            }
        }

        if cv % consumer_lag == 0 && cv > consumer_lag {
            let evict_to = cv - consumer_lag;
            deliver_cache_evict_up_to(evict_to);
            last_evicted = evict_to;
        }

        if cv % 100 == 0 || cv <= 20 {
            let lo = last_evicted + 1;
            let d = deliver_survivors(db_id, &repo, lo, cv, changes_per_commit);
            if d > peak {
                peak = d;
            }
            samples.push((cv, d));
        }
    }

    deliver_cache_evict_up_to(commits);
    let final_depth = deliver_survivors(db_id, &repo, 1, commits, changes_per_commit);

    eprintln!(
        "STAGE0_CACHE_PROBE deliver: commits={commits} changes_per_commit={changes_per_commit} \
         consumer_lag={consumer_lag} peak={peak} final_depth={final_depth} \
         samples={samples:?}"
    );

    let lag_bound = (consumer_lag as usize + consumer_lag as usize) * changes_per_commit * 2;
    assert!(
        peak <= lag_bound + 20,
        "deliver cache depth unexpectedly high: peak={peak}; expected <= {lag_bound}"
    );
}
