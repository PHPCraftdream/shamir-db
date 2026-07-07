//! Micro-bench for `RecordId::new()` — the hot ID-generation call on every
//! insert path.
//!
//! Migrated to the fixed-iteration harness ([`bench_scale_tool`]): the
//! workload knows nothing about its macro-iteration count — the harness owns
//! it (calibrated once into `bench-iters.txt`, then run as a static count so
//! run wall-time is a comparable speed signal).

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_types::types::record_id::RecordId;

fn main() {
    let mut h = Harness::new("record_id", env!("CARGO_MANIFEST_DIR"));
    h.bench("record_id_single/new", || {
        black_box(RecordId::new());
    });
    // Proof-of-concept for the setup/routine split: `Vec::with_capacity`
    // must be fresh every iteration (a shared Vec would just keep growing
    // and stop representing "push into an empty vec"), so this workload
    // uses `bench_batched` — only the `push` is timed, the fresh
    // allocation is not.
    h.bench_batched(
        "record_id_single/push_into_fresh_vec",
        Vec::<RecordId>::new,
        |mut v| {
            v.push(RecordId::new());
            black_box(&v);
        },
    );
    h.run();
}
