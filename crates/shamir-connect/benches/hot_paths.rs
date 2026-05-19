//! Hot-path benchmarks for `shamir-connect`.
//!
//! Run: `cargo bench -p shamir-connect --bench hot_paths`
//!
//! Groups (in order of "how often does this run on a busy server"):
//!
//! - `envelope` — RequestEnvelope / ResponseEnvelope msgpack encode + decode
//!   (every post-handshake request).
//! - `dispatch` — full server-side per-request path: parse → DashMap lookup →
//!   §7.5 validity check → handler → response encode.
//! - `session_store` — DashMap insert / lookup / remove (sub-µs target).
//! - `crypto_primitives` — HMAC-SHA256, SHA-256, HKDF-SHA256, AES-256-GCM
//!   encrypt+decrypt, Ed25519 sign+verify.
//! - `protocol_construction` — auth_message, identity_input, fake_blob,
//!   ticket encrypt+decrypt (per-handshake / per-resume).
//! - `handshake_verify` — full ServerHandshake::verify_proof for known and
//!   unknown user (~once per connection but sets P99 of the auth pipeline).
//!
//! Wire framing benchmarks live in `shamir-transport-tcp/benches/framing.rs`.
//!
//! Argon2id is intentionally NOT benchmarked — it's a deliberately slow
//! primitive (~2 s with default 128 MB params) and re-measuring it just
//! re-confirms the spec choice.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use shamir_connect::common::auth_message::{AuthMessage, AuthMessageInputs};
use shamir_connect::common::crypto::{
    aes256gcm_cipher, aes256gcm_decrypt, aes256gcm_decrypt_with_cipher, aes256gcm_encrypt,
    aes256gcm_encrypt_with_cipher, ed25519_verify_strict, hkdf_sha256, hmac_sha256, random_array,
    sha256, Ed25519Keypair, StoredKey,
};
use shamir_connect::server::ticket::{
    decrypt_ticket_with_ciphers, encrypt_ticket_with_cipher,
};
use shamir_connect::common::envelope::{
    RequestEnvelope, RequestEnvelopeRef, RequestEnvelopeView, ResponseEnvelope,
};
use shamir_connect::common::fake_blob::FakeBlob;
use shamir_connect::common::identity::build_identity_input;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, ProtocolVersion, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::config::{ListenerPolicy, ServerSecrets};
use shamir_connect::server::dispatch::{
    dispatch_request, dispatch_request_view, DispatchOutcome, RequestHandler,
};
use shamir_connect::server::handshake::{
    AuthInitView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_connect::server::ticket::{decrypt_ticket, encrypt_ticket, TicketPlain, TicketWire};
// (decrypt_ticket_with_ciphers / encrypt_ticket_with_cipher imported above)
use shamir_connect::server::user_record::UserRecord;

use zeroize::Zeroizing;

// ----------------------------------------------------------------------------
// Shared fixtures (built once outside the benchmarked region).
// ----------------------------------------------------------------------------

/// Faster-than-spec Argon2id params for *fixture construction only* — keeps
/// `cargo bench` startup short. The benchmarks under test do NOT include
/// Argon2id, so this only affects the one-time setup cost.
fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_auth_message() -> AuthMessage {
    let username = NormalizedUsername::from_raw("alice").unwrap();
    AuthMessage::build(AuthMessageInputs {
        username: &username,
        client_nonce: &[0xaa; 32],
        server_nonce: &[0xbb; 32],
        salt: &[0xcc; 16],
        kdf_params: KdfParams::DEFAULT,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &[0x77; 32],
        supported_version: ProtocolVersion::V1,
    })
    .unwrap()
}

fn make_user_record(password: &[u8]) -> UserRecord {
    let salt = [0x42u8; 16];
    let kdf = fast_kdf();
    let derived = DerivedKeys::derive(password, &salt, &kdf).unwrap();
    let mut sk = Zeroizing::new([0u8; 32]);
    sk.copy_from_slice(&derived.server_key[..]);
    UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: sk,
        kdf_params: kdf,
        tickets_invalid_before_ns: 0,
    }
}

fn fixture_session(uid: [u8; 16]) -> Session {
    Session::new(
        uid,
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0x77u8; 32],
        UnixNanos::now().as_u64(),
    )
}

// ----------------------------------------------------------------------------
// Group: envelope (msgpack encode/decode)
// ----------------------------------------------------------------------------

fn bench_envelope(c: &mut Criterion) {
    let mut g = c.benchmark_group("envelope");

    let sid = [0xa1u8; 32];
    for body_size in [16usize, 256, 4096].iter().copied() {
        let body = vec![0u8; body_size];
        let env = RequestEnvelope::new(sid, Some(42), body);
        let encoded = env.to_msgpack().unwrap();

        g.throughput(Throughput::Bytes(body_size as u64));
        g.bench_with_input(BenchmarkId::new("request_encode", body_size), &env, |b, e| {
            b.iter(|| {
                let bytes = e.to_msgpack().unwrap();
                black_box(bytes);
            });
        });

        // Optim #9: borrowed encode path — saves the per-call sid Vec<u8>
        // allocation that `RequestEnvelope::new` does.
        g.bench_with_input(
            BenchmarkId::new("request_encode_ref", body_size),
            &(sid, body_size),
            |b, (sid, sz)| {
                let body_buf = vec![0u8; *sz];
                b.iter(|| {
                    let r = RequestEnvelopeRef {
                        session_id: sid,
                        request_id: Some(42),
                        req: &body_buf,
                    };
                    let bytes = r.to_msgpack().unwrap();
                    black_box(bytes);
                });
            },
        );
        g.bench_with_input(
            BenchmarkId::new("request_decode", body_size),
            &encoded,
            |b, bytes| {
                b.iter(|| {
                    let parsed = RequestEnvelope::from_msgpack(bytes).unwrap();
                    black_box(parsed);
                });
            },
        );

        let resp = ResponseEnvelope::ok(Some(42), vec![0u8; body_size]);
        g.bench_with_input(BenchmarkId::new("response_encode", body_size), &resp, |b, r| {
            b.iter(|| {
                let bytes = r.to_msgpack().unwrap();
                black_box(bytes);
            });
        });
    }

    g.finish();
}

// ----------------------------------------------------------------------------
// Group: dispatch (full per-request server path)
// ----------------------------------------------------------------------------

struct EchoHandler;

impl RequestHandler for EchoHandler {
    fn handle(&self, _: &Session, req: &[u8]) -> Result<Vec<u8>, String> {
        Ok(req.to_vec())
    }
}

fn bench_dispatch(c: &mut Criterion) {
    let store = SessionStore::new();
    let sid = [0xa1u8; 32];
    let uid = [0x01u8; 16];
    store.insert(sid, fixture_session(uid));
    let handler = EchoHandler;

    let mut g = c.benchmark_group("dispatch");
    for body_size in [16usize, 256, 4096].iter().copied() {
        let env = RequestEnvelope::new(sid, Some(7), vec![0xcdu8; body_size]);
        // Pre-encoded msgpack bytes for the view path (simulates a wire
        // buffer freshly read by the framing layer).
        let env_bytes = env.to_msgpack().unwrap();

        g.throughput(Throughput::Elements(1));
        g.bench_with_input(BenchmarkId::new("happy_path", body_size), &env, |b, e| {
            b.iter(|| {
                let outcome = dispatch_request(e, &store, |_| 0u64, &handler).unwrap();
                let DispatchOutcome::Response(_) = outcome else {
                    panic!("expected response")
                };
            });
        });

        // Optim #4 — borrowed view path. Includes the msgpack PARSE step
        // (which the owning path also does inside `from_msgpack`); the saved
        // work is the per-field Vec<u8> allocations for sid + req.
        g.bench_with_input(
            BenchmarkId::new("happy_path_view", body_size),
            &env_bytes,
            |b, bytes| {
                b.iter(|| {
                    let view = RequestEnvelopeView::from_msgpack(black_box(bytes)).unwrap();
                    let outcome =
                        dispatch_request_view(&view, &store, |_| 0u64, &handler).unwrap();
                    let DispatchOutcome::Response(_) = outcome else {
                        panic!("expected response")
                    };
                });
            },
        );
    }
    g.finish();
}

// ----------------------------------------------------------------------------
// Group: session_store (raw DashMap operations)
// ----------------------------------------------------------------------------

fn bench_session_store(c: &mut Criterion) {
    let store = SessionStore::new();
    let sid = [0xa1u8; 32];
    let uid = [0x01u8; 16];
    store.insert(sid, fixture_session(uid));

    let mut g = c.benchmark_group("session_store");
    g.bench_function("lookup_hit", |b| {
        b.iter(|| {
            let s = store.lookup(black_box(&sid));
            black_box(s);
        });
    });
    // Optim #5 — lookup_at with caller-supplied timestamp (skips
    // UnixNanos::now() syscall inside the hot path).
    let captured_now = UnixNanos::now().as_u64();
    g.bench_function("lookup_at_hit", |b| {
        b.iter(|| {
            let s = store.lookup_at(black_box(&sid), captured_now);
            black_box(s);
        });
    });
    let miss_sid = [0xffu8; 32];
    g.bench_function("lookup_miss", |b| {
        b.iter(|| {
            let s = store.lookup(black_box(&miss_sid));
            black_box(s);
        });
    });
    // §7.5 validity check itself is a pure atomic compare; bench in isolation.
    let session_arc = store.lookup(&sid).unwrap();
    g.bench_function("validity_check_7_5", |b| {
        b.iter(|| {
            let ok = session_arc.is_valid_for_user(black_box(0u64));
            black_box(ok);
        });
    });
    // Per-session HMAC key derivation. `Session::hmac_key()` is called
    // by the server's destructive-op gate on every drop/clear op in a
    // batch. The cache version returns from an OnceLock after the
    // first call.
    let session_arc_for_hmac = store.lookup(&sid).unwrap();
    g.bench_function("hmac_key", |b| {
        b.iter(|| {
            let k = session_arc_for_hmac.hmac_key();
            black_box(k);
        });
    });

    g.finish();
}

// ----------------------------------------------------------------------------
// Group: crypto_primitives
// ----------------------------------------------------------------------------

fn bench_crypto_primitives(c: &mut Criterion) {
    let mut g = c.benchmark_group("crypto_primitives");

    // SHA-256
    for size in [32usize, 1024].iter().copied() {
        let data = vec![0xabu8; size];
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::new("sha256", size), &data, |b, d| {
            b.iter(|| {
                let h = sha256(black_box(d));
                black_box(h);
            });
        });
    }

    // HMAC-SHA256 — re-creating the Mac per call (current API).
    let key = [0x55u8; 32];
    let data = [0xabu8; 256];
    g.bench_function("hmac_sha256_256b", |b| {
        b.iter(|| {
            let tag = hmac_sha256(black_box(&key), black_box(&data));
            black_box(tag);
        });
    });

    // HKDF-SHA256 80 bytes (size used by FakeBlob).
    let ikm = [0x66u8; 32];
    let salt = [0x77u8; 19];
    let info = b"alice";
    g.bench_function("hkdf_sha256_80b", |b| {
        let mut out = [0u8; 80];
        b.iter(|| {
            hkdf_sha256(
                black_box(&ikm),
                black_box(&salt),
                black_box(info),
                &mut out,
            )
            .unwrap();
            black_box(&out);
        });
    });

    // AES-256-GCM encrypt + decrypt round-trip on a typical ticket plaintext.
    let aes_key = [0x88u8; 32];
    let nonce = [0x99u8; 12];
    let pt = vec![0xaau8; 256];
    let aad = b"SHAMIR-TICKET-v1\x01";
    let ct = aes256gcm_encrypt(&aes_key, &nonce, &pt, aad).unwrap();
    g.bench_function("aes256gcm_encrypt_256b", |b| {
        b.iter(|| {
            let out = aes256gcm_encrypt(black_box(&aes_key), &nonce, &pt, aad).unwrap();
            black_box(out);
        });
    });
    g.bench_function("aes256gcm_decrypt_256b", |b| {
        b.iter(|| {
            let out = aes256gcm_decrypt(black_box(&aes_key), &nonce, &ct, aad).unwrap();
            black_box(out);
        });
    });

    // Optim #3 — cached cipher avoids per-call key schedule.
    let cached_cipher = aes256gcm_cipher(&aes_key).unwrap();
    g.bench_function("aes256gcm_encrypt_256b_cached_cipher", |b| {
        b.iter(|| {
            let out = aes256gcm_encrypt_with_cipher(
                black_box(&cached_cipher),
                &nonce,
                &pt,
                aad,
            )
            .unwrap();
            black_box(out);
        });
    });
    g.bench_function("aes256gcm_decrypt_256b_cached_cipher", |b| {
        b.iter(|| {
            let out = aes256gcm_decrypt_with_cipher(
                black_box(&cached_cipher),
                &nonce,
                &ct,
                aad,
            )
            .unwrap();
            black_box(out);
        });
    });

    // Ed25519 sign + verify.
    let kp = Ed25519Keypair::generate();
    let pk = kp.public_bytes();
    let msg = vec![0u8; 200];
    let sig = kp.sign(&msg);
    g.bench_function("ed25519_sign_200b", |b| {
        b.iter(|| {
            let s = kp.sign(black_box(&msg));
            black_box(s);
        });
    });
    g.bench_function("ed25519_verify_strict_200b", |b| {
        b.iter(|| {
            let ok = ed25519_verify_strict(black_box(&pk), black_box(&msg), black_box(&sig));
            black_box(ok);
        });
    });

    g.finish();
}

// ----------------------------------------------------------------------------
// Group: protocol_construction
// ----------------------------------------------------------------------------

fn bench_protocol_construction(c: &mut Criterion) {
    let mut g = c.benchmark_group("protocol_construction");

    // auth_message build — 149 bytes for default params.
    let username = NormalizedUsername::from_raw("alice").unwrap();
    g.bench_function("auth_message_build", |b| {
        b.iter(|| {
            let am = AuthMessage::build(AuthMessageInputs {
                username: &username,
                client_nonce: black_box(&[0xaa; 32]),
                server_nonce: black_box(&[0xbb; 32]),
                salt: &[0xcc; 16],
                kdf_params: KdfParams::DEFAULT,
                transport_kind: TransportKind::Tcp,
                binding_mode: BindingMode::TlsExporter,
                tls_exporter_or_zeros: &[0x77; 32],
                supported_version: ProtocolVersion::V1,
            })
            .unwrap();
            black_box(am);
        });
    });

    // identity_input build (used per-handshake, contains auth_message).
    let am = make_auth_message();
    let pk = [0u8; 32];
    let session_id = [0u8; 32];
    g.bench_function("identity_input_build", |b| {
        b.iter(|| {
            let bytes = build_identity_input(
                black_box(&pk),
                TransportKind::Tcp,
                BindingMode::TlsExporter,
                &[0u8; 32],
                &am,
                &session_id,
                123_456_789,
            );
            black_box(bytes);
        });
    });

    // FakeBlob derive (per unknown-user handshake, runs HKDF-SHA256 80 B).
    let secret = [0xaau8; 32];
    g.bench_function("fake_blob_derive", |b| {
        b.iter(|| {
            let blob = FakeBlob::derive(black_box(&secret), &username).unwrap();
            black_box(blob);
        });
    });

    // Ticket encrypt + decrypt.
    let ticket_key = [0xddu8; 32];
    let plain = TicketPlain {
        version: 1,
        user_id: serde_bytes::ByteArray::new([0x01u8; 16]),
        username_nfc: "alice".into(),
        transport_kind_at_auth: 0x01,
        binding_mode_at_auth: 0x01,
        channel_binding_at_auth: serde_bytes::ByteArray::new([0x77u8; 32]),
        ticket_family_id: serde_bytes::ByteArray::new([0x11u8; 16]),
        original_auth_at_ns: 1_000_000,
        expires_at_ns: 2_000_000,
        family_counter: 1,
        roles: vec!["read_write".into()],
        identity_key_version: 0,
    };
    let wire = encrypt_ticket(&ticket_key, &plain).unwrap();
    g.bench_function("ticket_encrypt", |b| {
        b.iter(|| {
            let w = encrypt_ticket(&ticket_key, black_box(&plain)).unwrap();
            black_box(w);
        });
    });
    g.bench_function("ticket_decrypt", |b| {
        b.iter(|| {
            let p = decrypt_ticket(&ticket_key, None, black_box(&wire)).unwrap();
            black_box(p);
        });
    });

    // Optim #3 — pre-cached cipher (matches the production hot path
    // pattern: ResumeConfig holds the scheduled cipher across requests).
    let ticket_cipher = aes256gcm_cipher(&ticket_key).unwrap();
    g.bench_function("ticket_encrypt_cached_cipher", |b| {
        b.iter(|| {
            let w = encrypt_ticket_with_cipher(&ticket_cipher, black_box(&plain)).unwrap();
            black_box(w);
        });
    });
    g.bench_function("ticket_decrypt_cached_cipher", |b| {
        b.iter(|| {
            let p = decrypt_ticket_with_ciphers(&ticket_cipher, None, black_box(&wire)).unwrap();
            black_box(p);
        });
    });

    // Wire framing for ticket.
    let wire_bytes = wire.to_bytes();
    g.bench_function("ticket_wire_to_bytes", |b| {
        b.iter(|| {
            let bytes = wire.to_bytes();
            black_box(bytes);
        });
    });
    g.bench_function("ticket_wire_from_bytes", |b| {
        b.iter(|| {
            let w = TicketWire::from_bytes(black_box(&wire_bytes)).unwrap();
            black_box(w);
        });
    });

    g.finish();
}

// ----------------------------------------------------------------------------
// Group: handshake_verify (end-to-end server-side per-handshake)
// ----------------------------------------------------------------------------

fn bench_handshake_verify(c: &mut Criterion) {
    let mut g = c.benchmark_group("handshake_verify");

    let identity = Ed25519Keypair::generate();
    let secrets = ServerSecrets {
        server_secret: [0xaau8; 32],
        lockout_secret: [0xbbu8; 32],
    };
    let policy = ListenerPolicy::new(BindingMode::TlsExporter);
    let kdf = fast_kdf();
    let username = NormalizedUsername::from_raw("alice").unwrap();
    let password = b"correct horse battery staple";
    let user_record = make_user_record(password);

    // Pre-compute a valid client_proof so we measure ONLY server-side work.
    let derived = DerivedKeys::derive(password, &user_record.salt, &kdf).unwrap();
    let server_nonce_for_known = [0xbbu8; 32];
    let am_known = AuthMessage::build(AuthMessageInputs {
        username: &username,
        client_nonce: &[0xaau8; 32],
        server_nonce: &server_nonce_for_known,
        salt: &user_record.salt,
        kdf_params: kdf,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &[0x77u8; 32],
        supported_version: ProtocolVersion::V1,
    })
    .unwrap();
    // client_signature = HMAC(stored_key, am); client_proof = client_key XOR client_signature
    let sig = hmac_sha256(&derived.stored_key.0, am_known.as_bytes());
    let mut client_proof = [0u8; 32];
    for i in 0..32 {
        client_proof[i] = derived.client_key[i] ^ sig[i];
    }

    // The actual server_nonce is randomized by ServerHandshake — we cannot
    // bench `verify_proof` end-to-end with a real proof unless we control
    // the nonce. Instead, we measure `ServerHandshake::new()` (challenge
    // computation incl. fake_blob) and a known-bad proof through verify
    // (same code path, just rejection branch).

    // Bench: ServerHandshake::new + challenge for KNOWN user.
    let auth_init_known = AuthInitView {
        user: username.clone(),
        client_nonce: [0xaau8; 32],
        binding_mode: BindingMode::TlsExporter,
        version: 1,
    };
    let lookup_known = |_u: &NormalizedUsername| -> Option<UserRecord> {
        Some(user_record.clone())
    };
    g.bench_function("new_known_user", |b| {
        b.iter(|| {
            let hs = ServerHandshake::new(
                policy,
                TransportKind::Tcp,
                &secrets,
                auth_init_known.clone(),
                [0x77u8; 32],
                kdf,
                lookup_known,
            )
            .unwrap();
            let _ = black_box(hs.challenge());
        });
    });

    // Bench: ServerHandshake::new + challenge for UNKNOWN user (fake_blob path).
    let bob = NormalizedUsername::from_raw("bob").unwrap();
    let auth_init_unknown = AuthInitView {
        user: bob.clone(),
        client_nonce: [0xaau8; 32],
        binding_mode: BindingMode::TlsExporter,
        version: 1,
    };
    let lookup_unknown = |_u: &NormalizedUsername| -> Option<UserRecord> { None };
    g.bench_function("new_unknown_user_fake_blob_path", |b| {
        b.iter(|| {
            let hs = ServerHandshake::new(
                policy,
                TransportKind::Tcp,
                &secrets,
                auth_init_unknown.clone(),
                [0x77u8; 32],
                kdf,
                lookup_unknown,
            )
            .unwrap();
            let _ = black_box(hs.challenge());
        });
    });

    // Bench: full verify_proof for KNOWN user — the proof we built above
    // won't match because real handshake samples a fresh server_nonce
    // internally, BUT we measure the rejection-path cost (still HMAC + Ed25519
    // sign + identity_input build, which is what the constant-time discipline
    // ensures we always pay).
    g.bench_function("verify_proof_rejection_cost", |b| {
        b.iter(|| {
            let hs = ServerHandshake::new(
                policy,
                TransportKind::Tcp,
                &secrets,
                auth_init_known.clone(),
                [0x77u8; 32],
                kdf,
                lookup_known,
            )
            .unwrap();
            let outcome = hs.verify_proof(&client_proof, &identity, SESSION_MAX_AGE_NS).unwrap();
            black_box(outcome);
        });
    });

    // Same but for unknown user (always rejects) — bounds the fake_blob path.
    g.bench_function("verify_proof_unknown_user", |b| {
        b.iter(|| {
            let hs = ServerHandshake::new(
                policy,
                TransportKind::Tcp,
                &secrets,
                auth_init_unknown.clone(),
                [0x77u8; 32],
                kdf,
                lookup_unknown,
            )
            .unwrap();
            let outcome = hs.verify_proof(&client_proof, &identity, SESSION_MAX_AGE_NS).unwrap();
            matches!(outcome, ProofOutcome::Rejected)
                .then_some(())
                .expect("expected Rejected");
        });
    });

    g.finish();
}

// ----------------------------------------------------------------------------
// Misc helper: SCRAM derive (HMAC + SHA without Argon2id) — measures the
// "after Argon2id finishes" portion of the client side, useful as a baseline
// for "what does the rest of SCRAM cost beyond the password derivation".
// ----------------------------------------------------------------------------

fn bench_scram_post_argon(c: &mut Criterion) {
    let mut g = c.benchmark_group("scram_post_argon");

    // Pre-derive once outside the bench; we measure the HMAC+XOR portion.
    let salted = Zeroizing::new([0xaau8; 32]);
    let am = make_auth_message();

    g.bench_function("hmac_xor_proof_build", |b| {
        let stored = StoredKey([0xbbu8; 32]);
        let client_key = Zeroizing::new([0xccu8; 32]);
        b.iter(|| {
            let signature = hmac_sha256(&stored.0, am.as_bytes());
            let mut out = [0u8; 32];
            for i in 0..32 {
                out[i] = client_key[i] ^ signature[i];
            }
            black_box(out);
        });
    });

    // Server: recover client_key + recompute SHA256 + compare.
    g.bench_function("recover_client_key_and_compare", |b| {
        let stored = StoredKey([0xbbu8; 32]);
        let proof = [0xddu8; 32];
        b.iter(|| {
            let sig = hmac_sha256(&stored.0, am.as_bytes());
            let mut recovered = [0u8; 32];
            for i in 0..32 {
                recovered[i] = proof[i] ^ sig[i];
            }
            let recomputed = sha256(&recovered);
            let ok =
                shamir_connect::common::crypto::constant_time_eq(&recomputed, &stored.0);
            black_box(ok);
        });
    });

    // Avoid "unused" lint for `salted`.
    let _ = &salted;
    let _ = random_array::<32>();

    g.finish();
}

// ----------------------------------------------------------------------------
// Criterion entrypoint
// ----------------------------------------------------------------------------

fn bench_argon2_derive(c: &mut Criterion) {
    use shamir_connect::common::kdf_params::KdfParams;
    use shamir_connect::common::scram::DerivedKeys;

    let password = b"correct horse battery staple";
    let salt = [0xA1u8; 16];
    let kdf = KdfParams::DEFAULT;

    let mut g = c.benchmark_group("argon2_derive");
    g.sample_size(10);
    g.bench_function("default_params", |b| {
        b.iter(|| {
            criterion::black_box(DerivedKeys::derive(password, &salt, &kdf).unwrap());
        });
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_envelope,
    bench_dispatch,
    bench_session_store,
    bench_crypto_primitives,
    bench_protocol_construction,
    bench_handshake_verify,
    bench_scram_post_argon,
    bench_argon2_derive,
);
criterion_main!(benches);
