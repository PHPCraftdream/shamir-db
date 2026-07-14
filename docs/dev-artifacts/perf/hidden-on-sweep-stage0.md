בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Hidden-O(N) Sweep — Stage 0: measurements and routing decision

Stage 0 is the gate for the whole hidden-O(N) sweep campaign. Its job:
prove that the abstract suspicion ("`gc_upto` walks the full overlay
each drain pass") translates into actual cost in realistic operation —
or doesn't, in which case Stage 1 is gold-plating and gets deleted.

## P1 — synthetic `gc_upto` cost vs depth

Bench: `crates/shamir-tx/benches/overlay_gc_cost_vs_depth.rs` (quick
mode: 10 samples, 1s measurement, 1s warm-up).

### Full purge (remove everything)

| depth | time (median) | per-entry |
|---|---|---|
| 1,000 | 1.66 ms | 1.66 µs |
| 5,000 | 9.40 ms | 1.88 µs |
| 20,000 | 64.78 ms | 3.24 µs |

Mild superlinear scaling per entry — consistent with B+ tree depth +
cache effects. Throughput band 294-602 Kelem/s.

### Small-slice purge (remove only 100 lowest versions)

This is the **adversarial case** for the current full-iter implementation:
the GC pays for the whole tree to remove a tiny slice.

| depth | total time | per-removed | regression factor vs depth_1k |
|---|---|---|---|
| 1,000 | 472 µs | 4.7 µs/removed | 1× |
| 5,000 | 1.75 ms | 17.4 µs/removed | 3.7× |
| 20,000 | **6.57 ms** | **65.7 µs/removed** | **14×** |

**Verdict on the code shape: confirmed O(total depth), not O(removed).**
Removing 100 entries from a 20k overlay costs 14× more than removing
the same 100 from a 1k overlay. The smoking gun for `gc_upto`'s full
B+-tree iter (versioned_overlay.rs:154-187) is real.

## P2 — realistic overlay depth on a live repo

Probe: `crates/shamir-engine/tests/overlay_depth_probe.rs`. Four scenarios.

| probe | backend | scenario | peak depth | post-drain |
|---|---|---|---|---|
| A | InMemory | steady-state 10K commits | 2 | 0 |
| B | InMemory | tight burst 10K commits | 3 | 0 |
| C | InMemory | pinned snapshot + 5K commits | 1 | 1 |
| **D** | **fjall** (durable backend) | tight burst 2K commits | **1** | 0 |

The drainer keeps up tightly enough that overlay essentially never holds
more than a handful of entries — including on disk-backed fjall, which
pays real `history.set/transact` I/O per drain batch.

**This is by design.** Op #2 (incremental drainer cursor, ROI 82,
committed be0bc1f + 92311d4) made the drainer fast enough to keep the
overlay window bounded by `(durable_watermark, last_committed]`, and
that window is closed continuously on the background drainer's wake
loop. The pinned-snapshot probe (C) confirms the floor only constrains
`min_alive`, not `durable_watermark`, so `gc_overlay_to(durable_wm)`
still cleans the overlay regardless of held snapshots.

## P3 — subscription cache size

Skipped as a probe — the subscription `decode_cache` / `deliver_cache`
are **hash-based** (no implicit drain), so cache size is bounded by
client-side consumption rate, not by a backend property. Their retain-
based eviction at every watermark advance is therefore genuinely
O(cache_size) on every advance — Stage 2 stands on this structural
argument, no probe required.

## Routing decision

### Stage 1 (version-major secondary index in `VersionedOverlay`) — DELETED as gold-plating

The 14× cliff at depth 20k is **purely theoretical**: realistic depth
never exceeds 3 entries in any measured scenario, even on fjall under
sustained burst. Adding a `version_index: TreeIndex<(u64, Bytes), ()>`
would:

- **Double the memory per overlay entry** — a second fat-pointer Bytes
  key for every insert (refcount bump for the Arc, but still a heap
  allocation for the composite).
- **Add lock-free sync complexity** at three mutation sites
  (insert/remove/gc) that have to stay in lock-step under concurrent
  writers + the drainer.
- **Without measurable benefit** — at depth 1-3, both the current
  full-iter and a version-major range-drain cost the same handful of
  microseconds.

The existing TODO at versioned_overlay.rs:152-153 ("P1e may optimise
this with a version-major secondary index to avoid a full scan when the
tree is large relative to the GC batch") was correct in scoping the fix
to "when the tree is large". Op #2 ensures the tree is never large.

**The honest path forward:** if a future regression makes the overlay
grow (drainer health issue, new backpressure path, etc.), the right fix
is to restore drainer health, not optimize the GC. Stage 1 stays in the
roadmap as a contingent option, not as the immediate next step.

### Stage 2 (subscription cache: retain → range-drain) — PROCEED

`subscriptions/decode_cache.rs:94` and `deliver_cache.rs:94` do
`GLOBAL.inner.retain(|key, _| cv > up_to)` on every watermark advance.
Unlike the overlay, the subscription cache has no backend-side drain —
its size is bounded only by subscriber consumption rate, which under a
slow consumer can grow to thousands of entries. The retain-based
eviction is O(cache) per advance under this regime. Different shape,
real fix.

### Stage 3 (preventive guard against scc len()/count() in non-test code) — PROCEED

Structural, independent of overlay. scc's `TreeIndex::len()`,
`HashMap::len()`, `HashIndex::len()`, `HashCache::len()` are ALL O(N)
(verified against scc-2.4.0 source). We have been bitten twice (offer
backpressure, drain reseed-trigger). A grep-based audit + CLAUDE.md
note + atomic-mirror pattern prevents the third bite by construction.

## Files added (Stage 0 deliverables)

- `crates/shamir-tx/benches/overlay_gc_cost_vs_depth.rs` (new bench)
- `crates/shamir-tx/Cargo.toml` (dev-dep on shamir-bench-utils + `[[bench]]`)
- `crates/shamir-engine/tests/overlay_depth_probe.rs` (live-repo probes A/B/C/D)
- `docs/dev-artifacts/perf/hidden-on-sweep-stage0.md` (this document)

Gate: drainer 22/22 (still green from PT 1/PT 2), bench compiles and
runs in quick mode under the 60s wall-clock cap, probe tests 4/4 PASS.
