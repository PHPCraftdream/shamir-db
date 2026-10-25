//! Regression tests for `scalar_resolver` (P0 review cards 18A4B82E,
//! 4936C6ED, C8BC39BC).

use crate::registry::{v_int, v_str, FnEntry, ScalarResult};
use crate::scalar_resolver::{ScalarResolver, UserScalarLayer};
use shamir_types::types::value::QueryValue;

fn constant_entry(v: i64) -> FnEntry {
    FnEntry::pure(move |_| -> ScalarResult { Ok(v_int(v)) }, 0, None)
}

#[test]
fn rv_funclib_18a4b82e_register_replaces_existing_entry() {
    let layer = UserScalarLayer::new();
    layer.register("f", constant_entry(1));
    layer.register("f", constant_entry(2));
    let resolver = ScalarResolver::new(std::sync::Arc::new(layer));
    assert_eq!(resolver.call("f", &[]).unwrap(), v_int(2));
}

#[test]
fn rv_funclib_18a4b82e_user_layer_shadows_builtin_after_reregister() {
    let layer = UserScalarLayer::new();
    layer.register("math/abs", constant_entry(7));
    layer.register("math/abs", constant_entry(9));
    let resolver = ScalarResolver::new(std::sync::Arc::new(layer));
    assert_eq!(resolver.call("math/abs", &[]).unwrap(), v_int(9));
}

#[test]
fn rv_funclib_4936c6ed_resolver_dispatch_and_fallback() {
    // Unknown name → unknown_function; builtin fallback still works;
    // user-layer arity is honoured by the shared dispatch path.
    let layer = UserScalarLayer::new();
    layer.register(
        "strict",
        FnEntry::pure(
            |args: &[QueryValue]| -> ScalarResult { Ok(v_str(format!("n={}", args.len()))) },
            2,
            Some(3),
        ),
    );
    let resolver = ScalarResolver::new(std::sync::Arc::new(layer));
    assert_eq!(
        resolver.call("no_such_fn", &[]).unwrap_err().code,
        "unknown_function"
    );
    // Builtin fallback returns exactly what the builtin registry returns.
    assert_eq!(
        resolver.call("math/abs", &[QueryValue::Int(-5)]).unwrap(),
        crate::static_builtin()
            .call("math/abs", &[QueryValue::Int(-5)])
            .unwrap()
    );
    assert_eq!(resolver.call("strict", &[]).unwrap_err().code, "arity");
    // Exceeding `max_args` is an arity error too (upper bound is enforced).
    let four = [
        QueryValue::Int(1),
        QueryValue::Int(2),
        QueryValue::Int(3),
        QueryValue::Int(4),
    ];
    assert_eq!(resolver.call("strict", &four).unwrap_err().code, "arity");
    assert_eq!(
        resolver
            .call("strict", &[QueryValue::Int(1), QueryValue::Int(2)])
            .unwrap(),
        v_str("n=2".into())
    );
}

#[test]
fn rv_funclib_c8bc39bc_get_reads_registered_entry() {
    let layer = UserScalarLayer::new();
    assert!(layer.get("f").is_none());
    layer.register("f", constant_entry(3));
    assert!(layer.get("f").is_some());
    let resolver = ScalarResolver::new(std::sync::Arc::new(layer));
    assert!(resolver.get("f").is_some());
    assert!(resolver.get("missing").is_none());
}

/// Identity: the resolver returns the entry that was registered under the
/// name (a replaced registration returns the NEW one, builtins stay reachable
/// and a user entry shadows a builtin of the same name).
#[test]
fn rv_funclib_4936c6ed_resolver_get_returns_registered_identity() {
    let layer = UserScalarLayer::new();
    layer.register("f", constant_entry(1));
    layer.register("f", constant_entry(2));
    layer.register("math/abs", constant_entry(41));
    let resolver = ScalarResolver::new(std::sync::Arc::new(layer));
    let f = resolver.get("f").expect("registered");
    assert_eq!((f.f)(&[]).unwrap(), v_int(2));
    let shadow = resolver.get("math/abs").expect("shadowed builtin");
    assert_eq!((shadow.f)(&[]).unwrap(), v_int(41));
    let plain = ScalarResolver::new(std::sync::Arc::new(UserScalarLayer::new()));
    assert!(plain.get("math/abs").is_some(), "builtin reachable");
}
