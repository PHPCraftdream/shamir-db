use crate::{FnBatch, FnCtx, FunctionError, FunctionRegistry, Params};
use argon2::{Algorithm, Argon2, Params as Argon2Params, Version};
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn params(password: &[u8], salt: &[u8]) -> Params {
    let mut p = Params::new();
    p.set("password", QueryValue::Bin(password.to_vec()));
    p.set("salt", QueryValue::Bin(salt.to_vec()));
    p
}

/// Independent Argon2id reference using the function's documented defaults
/// (memory 19456 KiB, time 2, parallelism 1, length 32).
fn reference(password: &[u8], salt: &[u8]) -> Vec<u8> {
    let cfg = Argon2Params::new(19_456, 2, 1, Some(32)).unwrap();
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, cfg);
    let mut out = vec![0u8; 32];
    argon.hash_password_into(password, salt, &mut out).unwrap();
    out
}

#[tokio::test]
async fn argon2id_matches_reference_and_is_deterministic() {
    let reg = FunctionRegistry::with_builtins();
    let (ctx, batch) = (FnCtx::new(), FnBatch::new());
    let p = params(b"correct horse battery staple", b"0123456789abcdef");

    let out = reg.invoke("argon2id", &ctx, &batch, &p).await.unwrap();
    let bytes = match out {
        QueryValue::Bin(b) => b,
        other => panic!("expected Bin, got {other:?}"),
    };
    assert_eq!(bytes.len(), 32);
    assert_eq!(
        bytes,
        reference(b"correct horse battery staple", b"0123456789abcdef")
    );

    // Same inputs → same digest.
    let again = reg.invoke("argon2id", &ctx, &batch, &p).await.unwrap();
    assert_eq!(QueryValue::Bin(bytes), again);
}

#[tokio::test]
async fn argon2id_honours_custom_params() {
    let reg = FunctionRegistry::with_builtins();
    let mut p = params(b"pw", b"0123456789abcdef");
    p.set("length", QueryValue::Int(64));
    let out = reg
        .invoke("argon2id", &FnCtx::new(), &FnBatch::new(), &p)
        .await
        .unwrap();
    match out {
        QueryValue::Bin(b) => assert_eq!(b.len(), 64),
        other => panic!("expected Bin, got {other:?}"),
    }
}

#[tokio::test]
async fn argon2id_rejects_short_salt() {
    let reg = FunctionRegistry::with_builtins();
    let p = params(b"pw", b"short"); // < 8 bytes → Argon2 rejects
    let err = reg
        .invoke("argon2id", &FnCtx::new(), &FnBatch::new(), &p)
        .await
        .unwrap_err();
    assert!(matches!(err, FunctionError::Compute(_)), "got {err:?}");
}

#[tokio::test]
async fn argon2id_missing_param_is_error() {
    let reg = FunctionRegistry::with_builtins();
    let mut p = Params::new();
    p.set("password", QueryValue::Bin(b"pw".to_vec())); // no salt
    let err = reg
        .invoke("argon2id", &FnCtx::new(), &FnBatch::new(), &p)
        .await
        .unwrap_err();
    assert!(
        matches!(err, FunctionError::MissingParam(ref k) if k == "salt"),
        "got {err:?}"
    );
}

/// The KDF must run OFF the async worker thread. On a single-worker runtime
/// a separately-spawned task can only make progress while the worker is
/// free — which happens iff `argon2id` yields it by offloading to
/// `spawn_blocking`. If the hash ran inline on the worker, the spawned task
/// could not have run before `invoke` returned, and the flag would be unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn argon2id_does_not_block_the_async_worker() {
    let reg = FunctionRegistry::with_builtins();
    let ran = Arc::new(AtomicBool::new(false));
    let ran2 = ran.clone();

    let ticker = tokio::spawn(async move {
        ran2.store(true, Ordering::SeqCst);
    });

    let p = params(b"pw", b"0123456789abcdef");
    reg.invoke("argon2id", &FnCtx::new(), &FnBatch::new(), &p)
        .await
        .unwrap();

    assert!(
        ran.load(Ordering::SeqCst),
        "argon2id must offload to spawn_blocking and free the async worker"
    );
    ticker.await.unwrap();
}
