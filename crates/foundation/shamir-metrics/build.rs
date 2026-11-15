// Isolate the loom model-checker cfg to THIS crate only.
//
// The loom-gated code in this crate compiles under `cfg(loom)`. We emit that
// cfg HERE (gated on the `loom` cargo feature) rather than via a global
// `RUSTFLAGS="--cfg loom"`, so dependencies that carry their own
// `#[cfg(loom)]` blocks tied to a different loom API version are NOT dragged
// into their loom code paths (precedent: shamir-engine's build.rs).
//
// Run the loom model with:
//   cargo test -p shamir-metrics --features loom --lib tests::loom_model
fn main() {
    if std::env::var("CARGO_FEATURE_LOOM").is_ok() {
        println!("cargo::rustc-cfg=loom");
    }
}
