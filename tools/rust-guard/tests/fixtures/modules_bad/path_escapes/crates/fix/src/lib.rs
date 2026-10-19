// The `..` chain climbs out of the repository: crates/fix/src/../../../
// is ABOVE the repo root. The path must never be followed; the escaped
// spelling must not become a visited key (real-repo finding 2).
#[path = "../../../outside.rs"]
mod esc;
