//! What loading a module costs besides the compile: the admission pass (wasmparser
//! validation plus the limits) and the module hash, per size.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wasm_host::{admit_module, AdmissionLimits, AdmissionPolicy, ModuleHash};

use crate::compile::Sized;

pub fn register(h: &mut Harness, modules: &[Sized]) {
    let policy = Arc::new(AdmissionPolicy::Admission(AdmissionLimits::default()));
    for (id, bytes) in modules {
        let admitted = bytes.clone();
        let policy = policy.clone();
        h.bench(&format!("load/admit_{id}"), move || {
            black_box(admit_module(&admitted, &policy).expect("module is admitted"));
        });
        let hashed = bytes.clone();
        h.bench(&format!("load/hash_{id}"), move || {
            black_box(ModuleHash::of(&hashed));
        });
    }
}
