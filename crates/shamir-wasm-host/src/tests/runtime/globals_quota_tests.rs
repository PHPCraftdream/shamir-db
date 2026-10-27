use crate::runtime::context::AdmitProbe;
use crate::{GlobalVars, GlobalsQuota, GlobalsQuotaError};
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

fn quota(max_entries: usize, max_bytes: usize) -> GlobalsQuota {
    GlobalsQuota {
        max_entries,
        max_bytes,
        max_value_bytes: usize::MAX,
    }
}

#[test]
fn rv_wasm_host_51b448df_entry_cap_rejects_new_key_keeps_existing() {
    let vars = GlobalVars::with_quota(quota(1, usize::MAX));
    assert!(vars.try_set("a", QueryValue::Int(1), 1).is_ok());
    assert!(matches!(
        vars.try_set("b", QueryValue::Int(2), 1),
        Err(GlobalsQuotaError::Entries { .. })
    ));
    assert_eq!(vars.get("a"), Some(QueryValue::Int(1)));
    assert!(!vars.contains("b"));
}

#[test]
fn rv_wasm_host_51b448df_byte_cap_rejects_growth() {
    let reference = GlobalVars::new();
    reference.set("a", QueryValue::Int(1));
    let vars = GlobalVars::with_quota(quota(10, reference.bytes_used()));
    vars.set("a", QueryValue::Int(1));
    assert!(matches!(
        vars.try_set("a", QueryValue::Str("large".into()), 10),
        Err(GlobalsQuotaError::Bytes { .. })
    ));
    assert_eq!(vars.get("a"), Some(QueryValue::Int(1)));
}

#[test]
fn rv_wasm_host_51b448df_non_growing_overwrite_admitted_over_cap() {
    let vars = GlobalVars::with_quota(quota(1, 1));
    vars.set("a", QueryValue::Int(1));
    assert!(vars.try_set("a", QueryValue::Int(2), 1).is_ok());
    assert!(matches!(
        vars.try_set("b", QueryValue::Int(3), 1),
        Err(GlobalsQuotaError::Entries { .. })
    ));
}

#[test]
fn rv_wasm_host_51b448df_remove_refunds_capacity() {
    let vars = GlobalVars::with_quota(quota(1, usize::MAX));
    vars.set("a", QueryValue::Int(1));
    assert!(vars.remove("a"));
    assert!(vars.try_set("b", QueryValue::Int(2), 1).is_ok());
}

fn park_once() -> (AdmitProbe, Arc<AtomicUsize>, Arc<Barrier>, Arc<Barrier>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let probe_calls = calls.clone();
    let probe_entered = entered.clone();
    let probe_release = release.clone();
    let probe: AdmitProbe = Arc::new(move |_| {
        if probe_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            probe_entered.wait();
            probe_release.wait();
        }
    });
    (probe, calls, entered, release)
}

#[test]
fn rv_wasm_host_51b448df_reservation_precedes_publish() {
    let (probe, _, entered, release) = park_once();
    let vars = Arc::new(GlobalVars::with_quota(quota(1, usize::MAX)).with_admit_probe(probe));
    let writer = vars.clone();
    let thread = thread::spawn(move || writer.try_set("a", QueryValue::Int(1), 1));
    entered.wait();
    assert!(matches!(
        vars.try_set("b", QueryValue::Int(2), 1),
        Err(GlobalsQuotaError::Entries { .. })
    ));
    release.wait();
    assert!(thread.join().unwrap().is_ok());
    assert_eq!(vars.len(), 1);
}

#[test]
fn rv_wasm_host_51b448df_concurrent_overwrite_refunds_surplus() {
    let (probe, _, entered, release) = park_once();
    let setup = GlobalVars::with_quota(GlobalsQuota::default());
    let vars = Arc::new(setup.with_admit_probe(probe));
    let writer = vars.clone();
    let thread = thread::spawn(move || writer.try_set("a", QueryValue::Int(1), 1));
    entered.wait();
    vars.set("a", QueryValue::Str("much bigger".into()));
    release.wait();
    assert!(thread.join().unwrap().is_ok());
    let reference = GlobalVars::new();
    assert!(reference.try_set("a", QueryValue::Int(1), 1).is_ok());
    assert_eq!(vars.len(), 1);
    assert_eq!(vars.bytes_used(), reference.bytes_used());
    assert_eq!(vars.get("a"), Some(QueryValue::Int(1)));
}

#[test]
fn rv_wasm_host_51b448df_concurrent_remove_forces_retry() {
    let setup = GlobalVars::with_quota(GlobalsQuota::default());
    setup.set("a", QueryValue::Int(0));
    let (probe, calls, entered, release) = park_once();
    let vars = Arc::new(setup.with_admit_probe(probe));
    let writer = vars.clone();
    let thread = thread::spawn(move || writer.try_set("a", QueryValue::Int(1), 1));
    entered.wait();
    vars.remove("a");
    release.wait();
    assert!(thread.join().unwrap().is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(vars.len(), 1);
    let reference = GlobalVars::new();
    assert!(reference.try_set("a", QueryValue::Int(1), 1).is_ok());
    assert_eq!(vars.bytes_used(), reference.bytes_used());
}

#[test]
fn rv_wasm_host_51b448df_native_accounting_saturates() {
    let vars = GlobalVars::with_quota(quota(usize::MAX, usize::MAX));
    assert!(vars
        .try_set("a", QueryValue::Int(1), usize::MAX - 1)
        .is_ok());
    vars.set("b", QueryValue::Int(2));
    assert!(matches!(
        vars.try_set("c", QueryValue::Int(3), 1),
        Err(GlobalsQuotaError::Bytes { .. })
    ));
}

#[test]
fn rv_wasm_host_51b448df_update_charges_bytes() {
    let vars = GlobalVars::new();
    vars.update("k", |_| QueryValue::Str("big".into()));
    let reference = GlobalVars::new();
    reference.set("k", QueryValue::Str("big".into()));
    assert_eq!(vars.len(), 1);
    assert_eq!(vars.bytes_used(), reference.bytes_used());
}
