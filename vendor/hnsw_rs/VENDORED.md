# Vendored hnsw_rs 0.3.4

Source: crates.io `hnsw_rs` 0.3.4 (checksum in `Cargo.lock` before the patch:
`43a5258f079b97bf2e8311ff9579e903c899dcbac0d9a138d62e9a066778bd07`).
Only `src/`, the licences, README and Changes are kept; the examples, tests and
dev-dependencies of the upstream package are dropped.

## The one change

`src/hnsw.rs`, `reverse_update_neighborhood_simple`: the back-link of a new
point is written to layer `l` (the layer being updated) instead of the new
point's own top level. With the upstream line, a point of level >= 1 gets no
layer-0 back-links, parts of the graph become unreachable for layer-0 search
and recall collapses in roughly one build in a few hundred (#1232). Fixed
upstream in 0.3.5 (commit 6256b00b11).

## Removal

Wired through `[patch.crates-io]` in the root `Cargo.toml`. 0.3.5 was released
on 2026-10-09 and the 30-day dependency cooldown gate allows it from
2026-11-08. Then: delete this directory, the `[patch.crates-io]` entry and the
workspace `exclude` entry, and move the exact pin in `crates/shamir-index` to
`=0.3.5` after checking that the dump format is unchanged.
