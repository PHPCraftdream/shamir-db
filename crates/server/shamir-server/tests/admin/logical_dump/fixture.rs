//! Shared CI fixtures for the logical-dump tests (R3 step 4).
//!
//! One builder for a small database covering every supported stored kind,
//! and one builder for a deterministic ~64 MiB streaming dataset produced in
//! bounded batches (never held fully in memory). Everything goes through the
//! engine facade and the query builder only — never raw `serde_json`.
//!
//! Fixture-checklist compliance (every item failed CI on earlier steps):
//! persistent Fjall system store (`ShamirDb::init(SystemStoreConfig::Fjall(
//! root/"shamir_db_meta.redb"))`), `check_seed_catalogue` before any facade read,
//! repository `main` owned by the fixture user plus an explicit `chown`
//! grant on `store(db, "main")` (Manage is owner/admin-only), batches of at
//! most 50 queries with unique aliases, nested values built with
//! `QueryValue` (never `doc!`), minted record keys are canonical base58
//! `RecordId`s, and the returned engine handle is dropped by the caller
//! before the same directory is reopened (one tokio runtime per test, owned
//! by the caller through the local `runtime()` helper).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::user_record::UserRecord;
use shamir_db::access::{principal64, Actor};
use shamir_db::storage::storage_membuffer::MemBufferConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{chown, ResourceRef};
use shamir_query_builder::{doc, write};
use shamir_server::logical_dump::{self, LogicalExportArgs};
use shamir_server::logical_import::{self, ImportReport, LogicalImportArgs, LogicalImportError};
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::replication_seed::DumpReport;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

/// Directory name of the persistent (Fjall) system store under the fixture
/// root: the catalogue path the offline export and import drivers open.
pub const SYSTEM_STORE_DIR: &str = "shamir_db_meta.redb";

/// Directory name of the Fjall user directory under the fixture root.
pub const USER_DIRECTORY_DIR: &str = "users";

/// Fixture database name.
pub const DB_NAME: &str = "app";

/// Wire-level batch cap (query-builder `BatchLimits::default().max_queries`);
/// the fixtures stay safely below it.
pub const MAX_QUERIES_PER_BATCH: usize = 50;

/// Ops per batch used by the streaming builder (≤ [`MAX_QUERIES_PER_BATCH`]).
const STREAM_BATCH_OPS: usize = 25;

const _: () = assert!(STREAM_BATCH_OPS <= MAX_QUERIES_PER_BATCH);

/// Fixed seed of the streaming dataset's deterministic generator.
pub const STREAM_SEED: u64 = 0x5EED_D00D_C0FF_EE01;

/// Small records of the streaming dataset (`bulk/many`).
const STREAM_SMALL_RECORDS: usize = 7_000;

/// Padding string characters per small record (~8 KiB of text payload).
const STREAM_PAD_CHARS: usize = 8_192;

/// Deterministic binary tag bytes per small record.
const STREAM_TAG_BYTES: usize = 256;

/// Large values of the streaming dataset (`bulk/large`).
const STREAM_LARGE_RECORDS: usize = 40;

/// Binary payload of one large value (within the 512 KiB record binary
/// budget the export accepts).
const STREAM_BLOB_BYTES: usize = 256 * 1024;

/// Buffer TTL (ms) set on the indexed streaming table (`bulk/large`); the
/// exact-`ttl_ms` round-trip property test asserts this value survives.
pub const STREAM_TTL_MS: u64 = 2_500;

/// Buffer TTL (ms) set on the indexed small-fixture table.
const SMALL_TTL_MS: u64 = 5_000;

/// Current-thread runtime, mirroring the `logical_import/common.rs` helper
/// (that module is private to its folder, so the helper is mirrored here).
pub fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

/// System-store path of a fixture root.
pub fn system_store_path(root: &Path) -> PathBuf {
    root.join(SYSTEM_STORE_DIR)
}

/// Small fixture handle: the still-open engine (callers drop it before any
/// reopen of the same directories), the owner principal the catalogue rows
/// carry, and the built `(repository, table, enable_indexes, ttl_ms)` list.
pub struct SmallFixture {
    pub home: ShamirDb,
    pub owner_principal: u64,
    pub tables: Vec<(&'static str, &'static str, bool, Option<u64>)>,
}

/// Streaming fixture handle: same contract as [`SmallFixture`] plus the
/// exact deterministic payload byte count written.
pub struct StreamingFixture {
    pub home: ShamirDb,
    pub owner_principal: u64,
    pub tables: Vec<(&'static str, &'static str, bool, Option<u64>)>,
    pub payload_bytes: u64,
}

/// (a) Build a small multi-repository, multi-table database covering every
/// supported stored kind (null, bool, ints, floats, strings, lists, nested
/// maps, binary), `enable_indexes`, a non-default `ttl_ms`, identities and
/// ACL owners. Returns the open engine; callers drop it before exporting or
/// reopening the same directories.
pub async fn build_small_fixture(root: &Path) -> SmallFixture {
    let owner = create_owner_user(root).await;

    let home = ShamirDb::init(SystemStoreConfig::Fjall(system_store_path(root)))
        .await
        .unwrap();
    // Open every system-repository table before any facade read.
    home.check_seed_catalogue().await.unwrap();
    home.create_db_as(DB_NAME, Actor::User(owner))
        .await
        .unwrap();
    home.add_repo_as_with_durability(
        DB_NAME,
        shamir_db::engine::repo::RepoConfig::new(
            "main",
            shamir_db::engine::repo::BoxRepoFactory::fjall_raw(root.join("data_main")),
        ),
        Actor::User(owner),
        None,
    )
    .await
    .unwrap();
    home.add_repo_as_with_durability(
        DB_NAME,
        shamir_db::engine::repo::RepoConfig::new(
            "cold",
            shamir_db::engine::repo::BoxRepoFactory::fjall_raw(root.join("data_cold")),
        ),
        Actor::User(owner),
        None,
    )
    .await
    .unwrap();
    home.add_table_as(DB_NAME, "main", "rows", false, Actor::User(owner))
        .await
        .unwrap();
    home.add_table_as(DB_NAME, "main", "users", true, Actor::User(owner))
        .await
        .unwrap();
    home.add_table_as(DB_NAME, "cold", "events", false, Actor::User(owner))
        .await
        .unwrap();
    let instance = home.get_db(DB_NAME).unwrap();
    // The export read facade refuses unopened tables; open every fixture
    // table the way a booted server does before the engine is dropped.
    for (repo, table) in [("main", "rows"), ("main", "users"), ("cold", "events")] {
        instance.get_table(repo, table).await.unwrap();
    }
    instance
        .get_table("main", "users")
        .await
        .unwrap()
        .set_buffer_config(&MemBufferConfig {
            ttl_ms: Some(SMALL_TTL_MS),
            ..MemBufferConfig::default()
        })
        .await
        .unwrap();
    home.system_store()
        .save_setting(
            "root_meta",
            &doc! { "owner" => 0_i64, "mode" => 0o700_i64 }.into(),
        )
        .await
        .unwrap();

    // ACL ownership: Manage on the store is owner/admin-only, so grant the
    // fixture owner explicitly on `store("app", "main")` through the builder
    // (HMAC-gated op, hence `try_op`).
    let mut batch = Batch::new();
    batch
        .try_op(
            "grant_store_owner",
            chown(
                ResourceRef::Store {
                    store: [DB_NAME.to_string(), "main".to_string()],
                },
                owner,
            )
            .hmac("fixture"),
        )
        .unwrap();

    // Rich row: every supported stored kind, nested values built with
    // `QueryValue` (the `doc!` builder refuses nested lists/maps).
    let mut rich = new_map();
    rich.insert("n".to_string(), QueryValue::Null);
    rich.insert("flag".to_string(), QueryValue::Bool(true));
    rich.insert("off".to_string(), QueryValue::Bool(false));
    rich.insert("i_zero".to_string(), QueryValue::Int(0));
    rich.insert("i_pos".to_string(), QueryValue::Int(9_007_199_254_740_993));
    rich.insert("i_neg".to_string(), QueryValue::Int(-42));
    rich.insert("f".to_string(), QueryValue::F64(2.5));
    rich.insert("s".to_string(), QueryValue::Str("ambiguous".into()));
    rich.insert("bin".to_string(), QueryValue::Bin(vec![0, 0xff, 0x7f]));
    rich.insert(
        "l".to_string(),
        QueryValue::List(vec![
            QueryValue::Int(1),
            QueryValue::Str("two".into()),
            QueryValue::Null,
        ]),
    );
    let mut nested = new_map();
    nested.insert("z".to_string(), QueryValue::Int(1));
    nested.insert("a".to_string(), QueryValue::Str("x".into()));
    nested.insert("deep".to_string(), QueryValue::Bin(vec![1, 2, 3]));
    rich.insert("m".to_string(), QueryValue::Map(nested));
    batch.op(
        "put_rows_rich",
        write::Insert::with_repo("main", "rows").row(QueryValue::Map(rich)),
    );
    batch.op(
        "put_rows_second",
        write::Insert::with_repo("main", "rows").row(doc! { "s" => "second" }),
    );
    batch.op(
        "put_users_ada",
        write::Insert::with_repo("main", "users").row(doc! { "name" => "ada" }),
    );
    batch.op(
        "put_events_tick",
        write::Insert::with_repo("cold", "events").row(doc! { "kind" => "tick", "n" => 7_i64 }),
    );
    home.execute(DB_NAME, &batch.build()).await.unwrap();
    home.flush_all_waiting().await.unwrap();

    SmallFixture {
        home,
        owner_principal: owner,
        tables: vec![
            ("main", "rows", false, None),
            ("main", "users", true, Some(SMALL_TTL_MS)),
            ("cold", "events", false, None),
        ],
    }
}

/// (b) Build the deterministic ~64 MiB streaming dataset: repository
/// `bulk` with table `many` (7,000 small records of ~8 KiB text plus a
/// 256-byte binary tag) and table `large` (40 values of 256 KiB binary,
/// `enable_indexes`, non-default `ttl_ms`). Generated from
/// [`STREAM_SEED`] in bounded batches of [`STREAM_BATCH_OPS`] inserts with
/// periodic flushes, so no phase ever holds the dataset in memory. Record
/// keys are engine-minted canonical base58 `RecordId`s.
pub async fn build_streaming_fixture(root: &Path) -> StreamingFixture {
    build_streaming_fixture_scaled(root, STREAM_SMALL_RECORDS, STREAM_LARGE_RECORDS).await
}

/// The streaming builder at an arbitrary scale: `small_records` ~8 KiB
/// records and `large_records` 256 KiB blobs, generated identically in
/// bounded batches. The 1 GiB gated budget variant scales the record counts
/// by 16 through this entry point; nothing else changes.
pub async fn build_streaming_fixture_scaled(
    root: &Path,
    small_records: usize,
    large_records: usize,
) -> StreamingFixture {
    let owner = create_owner_user(root).await;

    let home = ShamirDb::init(SystemStoreConfig::Fjall(system_store_path(root)))
        .await
        .unwrap();
    home.check_seed_catalogue().await.unwrap();
    home.create_db_as(DB_NAME, Actor::User(owner))
        .await
        .unwrap();
    home.add_repo_as_with_durability(
        DB_NAME,
        shamir_db::engine::repo::RepoConfig::new(
            "bulk",
            shamir_db::engine::repo::BoxRepoFactory::fjall_raw(root.join("data_bulk")),
        ),
        Actor::User(owner),
        None,
    )
    .await
    .unwrap();
    home.add_table_as(DB_NAME, "bulk", "many", false, Actor::User(owner))
        .await
        .unwrap();
    home.add_table_as(DB_NAME, "bulk", "large", true, Actor::User(owner))
        .await
        .unwrap();
    let instance = home.get_db(DB_NAME).unwrap();
    for (repo, table) in [("bulk", "many"), ("bulk", "large")] {
        instance.get_table(repo, table).await.unwrap();
    }
    instance
        .get_table("bulk", "large")
        .await
        .unwrap()
        .set_buffer_config(&MemBufferConfig {
            ttl_ms: Some(STREAM_TTL_MS),
            ..MemBufferConfig::default()
        })
        .await
        .unwrap();

    let mut rng = Rng::new(STREAM_SEED);
    let mut pending = 0usize;
    let mut batch = Batch::new();
    for i in 0..small_records {
        let mut row = new_map();
        row.insert("id".to_string(), QueryValue::Int(i as i64));
        row.insert("seed".to_string(), QueryValue::Int(rng.next() as i64));
        row.insert("pad".to_string(), QueryValue::Str(pad_text(&mut rng)));
        row.insert("tag".to_string(), QueryValue::Bin(tag_bytes(&mut rng)));
        batch.op(
            format!("put_many_{i:06}"),
            write::Insert::with_repo("bulk", "many").row(QueryValue::Map(row)),
        );
        pending += 1;
        if pending == STREAM_BATCH_OPS {
            home.execute(DB_NAME, &batch.build()).await.unwrap();
            batch = Batch::new();
            pending = 0;
            // Keep the buffer from accumulating the dataset in memory.
            home.flush_all_waiting().await.unwrap();
        }
    }
    for i in 0..large_records {
        let mut row = new_map();
        row.insert("id".to_string(), QueryValue::Int(i as i64));
        row.insert("blob".to_string(), QueryValue::Bin(blob_bytes(&mut rng)));
        batch.op(
            format!("put_large_{i:04}"),
            write::Insert::with_repo("bulk", "large").row(QueryValue::Map(row)),
        );
        pending += 1;
        if pending == STREAM_BATCH_OPS {
            home.execute(DB_NAME, &batch.build()).await.unwrap();
            batch = Batch::new();
            pending = 0;
            home.flush_all_waiting().await.unwrap();
        }
    }
    if pending > 0 {
        home.execute(DB_NAME, &batch.build()).await.unwrap();
    }
    home.flush_all_waiting().await.unwrap();

    StreamingFixture {
        home,
        owner_principal: owner,
        tables: vec![
            ("bulk", "many", false, None),
            ("bulk", "large", true, Some(STREAM_TTL_MS)),
        ],
        payload_bytes: (small_records * (STREAM_PAD_CHARS + STREAM_TAG_BYTES)
            + large_records * STREAM_BLOB_BYTES) as u64,
    }
}

/// Create the `users/` directory with one enabled user "owner" and return
/// the principal64 id the catalogue stores for it (same shape as
/// `logical_import/common.rs`; that helper is private to its module).
async fn create_owner_user(root: &Path) -> u64 {
    let directory = FjallUserDirectory::open(root.join(USER_DIRECTORY_DIR)).unwrap();
    let user_id = directory
        .insert_with_scope("owner".to_string(), user_record(), None)
        .unwrap();
    principal64(user_id)
}

fn user_record() -> UserRecord {
    UserRecord {
        salt: [7; 16],
        stored_key: StoredKey([8; 32]),
        server_key: Zeroizing::new([9; 32]),
        kdf_params: KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

/// Deterministic xorshift64* — no external dependency, fixed seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

const ALPHABET: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";

fn pad_text(rng: &mut Rng) -> String {
    let mut text = String::with_capacity(STREAM_PAD_CHARS);
    for _ in 0..STREAM_PAD_CHARS {
        text.push(ALPHABET[(rng.next() >> 24) as usize % ALPHABET.len()] as char);
    }
    text
}

fn tag_bytes(rng: &mut Rng) -> Vec<u8> {
    (0..STREAM_TAG_BYTES)
        .map(|_| (rng.next() >> 16) as u8)
        .collect()
}

fn blob_bytes(rng: &mut Rng) -> Vec<u8> {
    (0..STREAM_BLOB_BYTES)
        .map(|_| (rng.next() >> 16) as u8)
        .collect()
}

/// Scope options for the offline export driver (`--db/--output/--repo/
/// --table/--schema-only/--include-artifacts`), mirroring the private
/// `logical_import/common.rs` builder.
pub fn export_args(db: &str, output: &Path, schema_only: bool) -> LogicalExportArgs {
    LogicalExportArgs {
        db: db.into(),
        output: output.into(),
        repo: None,
        table: None,
        schema_only,
        include_artifacts: false,
    }
}

/// Options for the offline import driver (`--input/--target/--schema-only/
/// --as-db`), mirroring the private `logical_import/common.rs` builder.
pub fn import_args(input: &Path, target: &Path) -> LogicalImportArgs {
    LogicalImportArgs {
        input: input.into(),
        target: target.into(),
        schema_only: false,
        as_db: None,
    }
}

/// Drive the offline export driver synchronously on a fresh runtime; the
/// fixture database must already be closed.
pub fn export_now(root: &Path, args: &LogicalExportArgs) -> DumpReport {
    runtime().block_on(export_waiting_for_locks(root, args))
}

/// Drive the offline export driver on the caller's runtime, the one that
/// built the fixture: background tasks of the dropped engine still hold the
/// Fjall directory locks until that runtime runs them, so a streaming test
/// exports on its own runtime instead of a fresh one.
pub fn export_on(rt: &Runtime, root: &Path, args: &LogicalExportArgs) -> DumpReport {
    rt.block_on(export_waiting_for_locks(root, args))
}

/// Upper bound for a dropped fixture engine to hand its Fjall directory locks
/// back, and the pause between two export attempts.
const LOCK_RELEASE_BUDGET: Duration = Duration::from_secs(30);
const LOCK_RELEASE_POLL: Duration = Duration::from_millis(50);

/// Export; a `Locked` refusal means the dropped fixture engine has not
/// released its directory locks yet (its background threads and tasks exit
/// asynchronously, slowly on a loaded runner), so the attempt is repeated
/// within [`LOCK_RELEASE_BUDGET`]. The pause runs on the caller's runtime,
/// which also lets the dropped engine's tasks finish. Opening fails before
/// anything is written, so a repeat is safe; any other error fails at once.
pub async fn export_waiting_for_locks(root: &Path, args: &LogicalExportArgs) -> DumpReport {
    let deadline = Instant::now() + LOCK_RELEASE_BUDGET;
    loop {
        match logical_dump::export(root, args, CancellationToken::new()).await {
            Ok(report) => return report,
            Err(error) if format!("{error:#}").contains("Locked") && Instant::now() < deadline => {
                tokio::time::sleep(LOCK_RELEASE_POLL).await;
            }
            Err(error) => panic!("logical export failed: {error:#}"),
        }
    }
}

/// Drive the offline import driver synchronously and keep the typed result.
pub fn import_now(
    args: &LogicalImportArgs,
    cancel: CancellationToken,
) -> Result<ImportReport, LogicalImportError> {
    runtime().block_on(logical_import::import(args.clone(), cancel))
}

/// Sibling stage prefix written beside the target by `logical-import`.
const STAGE_PREFIX: &str = ".logical-import-";

/// True when a stage directory of a running import attempt is visible in
/// `root` (the deterministic mid-load cancellation handshake).
pub fn stage_directory_present(root: &Path) -> bool {
    fs::read_dir(root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(STAGE_PREFIX)
    })
}

/// Nothing of a failed or cancelled attempt survives: the target is absent,
/// the lock file is gone and no stage directory remains beside the target.
pub fn assert_no_stage_leftovers(root: &Path, target: &Path) {
    assert!(
        !target.exists(),
        "target {} must not exist",
        target.display()
    );
    let lock = root.join(format!(
        "{}.import.lock",
        target.file_name().unwrap().to_string_lossy()
    ));
    assert!(!lock.exists(), "lock {} must be removed", lock.display());
    assert!(
        !stage_directory_present(root),
        "a stage directory was left behind"
    );
}
