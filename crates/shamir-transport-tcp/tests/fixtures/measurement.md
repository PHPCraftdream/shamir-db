# CI-only plain-TCP framing measurement recipe

Use a dedicated Linux CI runner with a fixed kernel image, CPU allocation and
Rust toolchain. Record the image digest, kernel, CPU model, toolchain version,
commit, and runner load with each result. Do not compare runs from different
runner configurations. This recipe is not a local benchmark instruction.

On an isolated runner, use a purpose-built plain TCP loopback sender and receiver
that call `write_frame_with_cap` and `read_frame` respectively (no TLS). For
payload sizes 1, 64, 1024, 65536 and 1048576 bytes, send a fixed count of
frames after a warm-up; verify every received payload before recording results.
Record the count and duration, plus p50/p95/p99 per-frame round-trip latency
from a monotonic clock. Run multiple independent repetitions and report the
raw samples and median. Pin the sender and receiver to documented CPUs, and
record socket options, frame count, warm-up, and CPU governor.

In a separate run with identical payloads and configuration, use Linux `strace
-f -c` filtered to `write`, `writev`, `sendto`, `sendmsg`, `read`, `readv`,
`recvfrom`, and `recvmsg` to collect per-process syscall counts. Report syscall
counts per frame separately for sender and receiver and retain the trace summary.
Tracing perturbs latency: never use the traced run for latency figures.

The sender/receiver harness IS implemented as the fixed-iteration bench
`plain_tcp/framing_round_trip/<size>` (file
`crates/shamir-transport-tcp/benches/measurement.rs`, `bench_scale_tool`
harness with the `async` feature). CI invokes it on the isolated runner
described above with
`cargo bench -p shamir-transport-tcp --bench measurement` (using the
isolated bench target directory convention from CLAUDE.md), and captures
the deterministic per-size result lines printed to stdout as the CI
artifact; percentile computation, if wanted, happens on that recorded
output, not inside the bench. **No numeric results are recorded yet — CI
supplies them.** Latency is a trend, not a deterministic pass/fail
threshold; do not claim a performance improvement from this recipe alone.
