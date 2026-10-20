//! Shared fixtures for the cursor handler tests: sessions, handler
//! builder, wire-request constructors, row/score seeding helpers and the
//! pinned-mode peek helper.


use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{chmod};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::ResourceRef;
use shamir_query_types::read::{
    ReadQuery,
};
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use crate::cursor_registry::PaginationMode;
use crate::db_handler::config::CursorLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

pub(super) const ALICE_SID: [u8; 32] = [0xAA; 32];
pub(super) const BOB_SID: [u8; 32] = [0xBB; 32];

/// `Session::new` does not take `session_id` as a constructor argument (it
/// always zero-inits that field; the 6th arg is `channel_binding_at_auth`,
/// a DIFFERENT field) — real sessions get theirs assigned post-construction
/// by the resume/auth path (see `Session::new_authenticated`/`resume` in
/// `shamir-connect`). Tests that need two DISTINCT sessions (for the
/// cross-session ownership / per-session-cap tests below) must set
/// `session_id` explicitly on the public field, or every fixture session
/// collapses to the same all-zero id.
pub(super) fn alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

pub(super) fn other_session() -> Session {
    let mut s = Session::new(
        [0xCD; 16],
        "bob".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = BOB_SID;
    s
}

/// Build a handler over an in-memory `ShamirDb` with `app.main.items`,
/// owned by alice (mirrors `node_mode_tests::build_handler`), with
/// `n` rows pre-inserted (`{ id: "kNN", v: NN }`).
pub(super) async fn build_handler_with_rows(n: usize, cursor_limits: CursorLimitsCap) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    if n > 0 {
        let mut b = Batch::new();
        for i in 0..n {
            b.insert(
                format!("i{i}"),
                insert("items").row(doc! { "id" => format!("k{i:03}"), "v" => i as i64 }),
            );
        }
        let batch = b.build();
        shamir
            .execute_as(owner, "app", &batch)
            .await
            .expect("seed rows");
    }

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(cursor_limits)
}

pub(super) async fn send(handler: &ShamirDbHandler, session: &Session, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let resp_bytes = handler
        .handle(session, &bytes, &conn)
        .await
        .expect("handle must not error at the protocol level");
    rmp_serde::from_slice(&resp_bytes).expect("decode response")
}

pub(super) fn create_cursor_req(query: ReadQuery, page_size: u32) -> DbRequest {
    DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        query,
        page_size,
    }
}

/// Explicit per-call `page_size` override (the pre-CR-B3 always-present
/// shape) — wraps `Some(page_size)` so the many existing call sites below
/// don't need to change.
pub(super) fn fetch_next_req(cursor_id: CursorId, page_size: u32) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id,
        page_size: Some(page_size),
    }
}

/// CR-B3 (#769): `FetchNext` that OMITS `page_size` — server falls back to
/// the cursor's stored `CreateCursor`-time default.
pub(super) fn fetch_next_default_req(cursor_id: CursorId) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id,
        page_size: None,
    }
}

/// Open `app.main.items` to `mode` at all three ancestor levels (database,
/// store, table) — `authorize_access`'s ancestor-walk requires `Execute` on
/// EVERY ancestor, not just the target, so a non-owner needs all three
/// chmod'd (mirrors `permission_e2e.rs`'s
/// `permission_open_default_allows_any_user` "chmod db + repo + table to
/// OPEN" sequence).
pub(super) async fn open_app_main_items(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "db",
        chmod(
            ResourceRef::Database {
                database: "app".into(),
            },
            mode,
        ),
    );
    batch.chmod(
        "store",
        chmod(
            ResourceRef::Store {
                store: ["app".into(), "main".into()],
            },
            mode,
        ),
    );
    batch.chmod(
        "table",
        chmod(
            ResourceRef::Table {
                table: ["app".into(), "main".into(), "items".into()],
            },
            mode,
        ),
    );
    handler
        .db()
        .execute_as(Actor::System, "app", &batch.build())
        .await
        .expect("admin chmod must succeed");
}

// ---------------------------------------------------------------------------
// Happy path: CreateCursor -> FetchNext (repeatable, multiple pages) ->
// CancelCursor, has_more transitions true -> false, post-exhaustion fetch
// is a clean error (not a panic).
// ---------------------------------------------------------------------------

/// F-1 (#792): register a `SchemaValidator` declaring `field` as `ty`
/// (`required`), and bind it to `db_name.repo_name.table_name` — the minimal
/// setup that makes `order_by_column_is_schema_typed_scalar` prove a column
/// homogeneous so a cursor over it can reach `PaginationMode::Keyset` at all.
/// F-17 (#810) additionally requires the matched rule's `keyset_safe` proof
/// to be `true`; this helper is called BEFORE any row is seeded, so the
/// `FieldRule` below is constructed with `keyset_safe: true` directly
/// (mirroring what `stamp_keyset_safe` would compute at bind time on a
/// genuinely empty table), keeping every existing Keyset-mode test path
/// unchanged.
///
/// Registers the `SchemaValidator` DIRECTLY into the live `ValidatorRegistry`
/// (`db.validators().register(...)`) rather than via `ShamirDb::
/// register_native_validator` — that facade wraps the validator in a
/// `NativeRecordValidator` closure (see `crates/shamir-db/tests/
/// declarative_schema_e2e.rs`'s `register_and_bind_schema`), which would lose
/// the concrete `SchemaValidator` type this task's new
/// `RecordValidator::as_schema_rules` downcast hook needs to see. Registering
/// the compiled `SchemaValidator` directly (mirroring `ValidatorRegistry::
/// register`'s own doc comment: "Look up a compiled validator by id") skips
/// only the catalogue-row persistence step, which none of these in-memory,
/// single-process tests need.
pub(super) async fn bind_schema(
    db: &ShamirDb,
    db_name: &str,
    repo_name: &str,
    table_name: &str,
    field: &str,
    ty: shamir_db::engine::validator::schema::TypeTag,
) {
    use shamir_db::engine::validator::schema::{Constraints, FieldRule, SchemaValidator};
    use shamir_db::engine::validator::WriteOp;
    use shamir_types::types::record_id::RecordId;

    let field_rule = FieldRule {
        path: vec![field.to_string()],
        ty,
        constraints: Constraints {
            required: true,
            ..Constraints::default()
        },
        // F-17 (#810): schema bound BEFORE rows are seeded — the table is
        // empty, so the column IS provably homogeneous by construction.
        keyset_safe: true,
    };
    let schema = SchemaValidator::new(vec![field_rule]);
    let id = RecordId::new();
    db.validators()
        .register(id, format!("{table_name}_{field}_schema"), Arc::new(schema))
        .expect("register schema validator");
    db.bind_validator(
        db_name,
        repo_name,
        table_name,
        &format!("{table_name}_{field}_schema"),
        vec![WriteOp::Insert, WriteOp::Update, WriteOp::Upsert],
        1000,
    )
    .await
    .expect("bind schema validator");
}

/// Build a handler over an in-memory `ShamirDb` with `app.main.items`, owned
/// by alice, seeded with one row per entry in `scores`: `{ seq: i, score:
/// scores[i] }`. `seq` is a strictly-increasing insertion-order tiebreaker
/// field distinct from `score`, letting tests identify every row uniquely
/// even when many rows share the same `score`.
///
/// F-1 (#792): binds a schema declaring `score: Int, required` so these
/// tests keep exercising genuine `PaginationMode::Keyset` mechanics (tie-run
/// bookmarks, CR-D1's ceiling fallback, etc.) — every caller of this helper
/// predates the schema-typed-scalar gate and its whole point is Keyset-mode
/// coverage, not the schemaless-conservative-default this task adds (that
/// default has its own dedicated tests, see `order_by_column_is_schema_typed_
/// scalar`'s regression tests below). F-17 (#810): `bind_schema` stamps
/// `keyset_safe: true` (table is empty at this point), so the gate still
/// passes and Keyset mode is reached exactly as before.
pub(super) async fn build_handler_with_scores(
    scores: &[i64],
    cursor_limits: CursorLimitsCap,
) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");
    bind_schema(
        &shamir,
        "app",
        "main",
        "items",
        "score",
        shamir_db::engine::validator::schema::TypeTag::Int,
    )
    .await;

    if !scores.is_empty() {
        let mut b = Batch::new();
        for (i, score) in scores.iter().enumerate() {
            b.insert(
                format!("i{i}"),
                insert("items").row(doc! { "seq" => i as i64, "score" => *score }),
            );
        }
        let batch = b.build();
        shamir
            .execute_as(owner, "app", &batch)
            .await
            .expect("seed rows");
    }

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(cursor_limits)
}

/// Drain a cursor (already created, first page already consumed by the
/// caller) via repeated `FetchNext(page_size)` calls until `has_more ==
/// false`, collecting every page's rows' `seq` values (via
/// `get_value_i64("seq")`) into the given accumulator.
pub(super) async fn drain_seqs(
    handler: &ShamirDbHandler,
    session: &Session,
    cursor_id: CursorId,
    page_size: u32,
    out: &mut Vec<i64>,
) {
    loop {
        let resp = send(handler, session, fetch_next_req(cursor_id, page_size)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    out.push(
                        r.get_value_i64("seq")
                            .expect("every row must carry a seq field"),
                    );
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage while draining, got {other:?}"),
        }
    }
}


/// Peek a live cursor's pinned `PaginationMode` directly off the registry --
/// the test-visible signal the brief asks for (mirrors
/// `keyset_pagination_mode_pinned_at_creation_stays_stable`'s doc comment,
/// which notes no wire-visible field exposes this, so tests must reach
/// in-process). `Cursor::state()` is a `pub` `tokio::sync::Mutex`, and
/// nothing else is concurrently touching this cursor mid-assertion in these
/// single-threaded-per-cursor tests, so a non-blocking `try_lock` is safe
/// and avoids making this helper `async` for no reason.
pub(super) fn pinned_mode(handler: &ShamirDbHandler, cursor_id: CursorId, sid: &[u8; 32]) -> PaginationMode {
    let cursor = handler
        .cursor_registry
        .get_owned(cursor_id.0, sid)
        .expect("cursor must still be registered");
    let mode = cursor.state().try_lock().expect("uncontended in test").mode;
    mode
}
