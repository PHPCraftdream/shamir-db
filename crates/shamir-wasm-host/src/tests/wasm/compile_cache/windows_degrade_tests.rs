//! Non-Unix (Windows) degradation: the disk tier is disabled there, so
//! `with_compile_cache` must still yield a working (uncached) engine.

use crate::wasm::WasmEngine;

const MODULE: &str = r#"
(module
  (func (export "f") (result i32) i32.const 1))
"#;

#[test]
fn with_compile_cache_degrades_to_an_uncached_engine() {
    let dir = tempfile::tempdir().unwrap();
    let engine = WasmEngine::with_compile_cache(dir.path().to_path_buf())
        .expect("cache must be silently disabled, never fail");
    let wasm = wat::parse_str(MODULE).unwrap();
    assert!(engine.compile(&wasm).is_ok(), "engine must still compile");
}
