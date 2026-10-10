//! Concurrent inserts into one `hnsw_rs` 0.3.4 graph must not deadlock (#1232).
//!
//! `Hnsw::reverse_update_neighborhood_simple` iterates
//! `new_point.neighbours.read()` for its WHOLE loop and, inside it, takes
//! `q.neighbours.write()` for each neighbour `q`. Two points inserted
//! concurrently that picked each other as neighbours therefore hold
//! `P.neighbours` (read) while waiting for `Q.neighbours` (write) and vice
//! versa — an ABBA cycle on `parking_lot` locks that never resolves. The
//! window is tiny for well-spread data and wide for tightly clustered data,
//! where nearly every concurrent insert picks the others as neighbours.
//!
//! Every graph build in the adapter (`parallel_insert` in `upsert_batch` and
//! the SQ8 fit, plain `insert` from concurrent `spawn_blocking` upserts) is
//! exposed. The patched copy of `hnsw_rs` under `vendor/hnsw_rs` snapshots the
//! neighbour list before taking any write lock.

use hnsw_rs::anndists::dist::distances::DistL2;
use hnsw_rs::hnsw::Hnsw;
use std::sync::mpsc;
use std::time::Duration;

const ROUNDS: usize = 6;
const POINTS: usize = 3000;
const DIM: usize = 8;
const THREADS: usize = 8;
const ROUND_TIMEOUT: Duration = Duration::from_secs(120);

/// Near-identical points (jitter ~1e-5): every insert selects the in-flight
/// points as neighbours, so mutual-neighbour pairs are the norm.
fn tight_cluster(n: usize) -> Vec<Vec<f32>> {
    (0..n)
        .map(|i| {
            (0..DIM)
                .map(|d| 1.0 + ((i * (d + 3)) % 17) as f32 * 1e-5)
                .collect()
        })
        .collect()
}

#[test]
fn concurrent_insert_of_mutual_neighbours_does_not_deadlock() {
    let (done_tx, done_rx) = mpsc::channel::<usize>();

    // Run in a detached thread so a deadlock fails the test with a named
    // message instead of hanging the whole nextest process.
    std::thread::spawn(move || {
        for round in 0..ROUNDS {
            let hnsw = Hnsw::<f32, DistL2>::new(16, POINTS, 16, 64, DistL2 {});
            let data = tight_cluster(POINTS);
            let chunk = POINTS.div_ceil(THREADS);
            std::thread::scope(|scope| {
                for t in 0..THREADS {
                    let hnsw = &hnsw;
                    let part = &data[(t * chunk).min(POINTS)..((t + 1) * chunk).min(POINTS)];
                    scope.spawn(move || {
                        for (j, v) in part.iter().enumerate() {
                            hnsw.insert((v.as_slice(), t * chunk + j));
                        }
                    });
                }
            });
            if done_tx.send(round).is_err() {
                return;
            }
        }
    });

    for round in 0..ROUNDS {
        done_rx.recv_timeout(ROUND_TIMEOUT).unwrap_or_else(|_| {
            panic!(
                "hnsw_rs concurrent insert deadlocked in round {round} \
                 (reverse_update_neighborhood_simple ABBA on neighbour locks)"
            )
        });
    }
}
