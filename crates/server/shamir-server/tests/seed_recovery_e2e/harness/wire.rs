//! Leader-side wire drivers: schema/publication/replicator setup, typed
//! writes (insert / update / delete through the query builder — never raw
//! JSON), full row-set reads, and the Hello gap-certificate snapshot.

use std::net::SocketAddr;

use shamir_client::Client;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::filter::eq;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_types::admin::CreatePublicationOp;
use shamir_query_types::wire::repl::{ReplRepoInfo, ReplRequest, ReplResponse};
use shamir_types::types::value::QueryValue;
use zeroize::Zeroizing;

use super::child;

pub const DB: &str = "app";
pub const REPO: &str = "main";
pub const TABLES: [&str; 2] = ["items", "audit"];
pub const PUBLICATION: &str = "pub";

/// The source gap certificate as advertised by `ReplHello` — the identity
/// (`lineage`, `epoch`, `schema`) plus the certified recovery interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapCert {
    pub lineage: [u8; 16],
    pub epoch: u64,
    pub gap_first: Option<u64>,
    pub gap_through: Option<u64>,
    pub schema: String,
}

/// Create db / repo / tables, OPEN the access path for the replicator,
/// create the `replicator` account, and publish `pub` scoped to app/main.
pub async fn setup_leader(admin: &Client) {
    let mut batch = Batch::new();
    batch.id("setup");
    batch.create_db("db", ddl::create_db(DB));
    // A database is created from the always-present `default` context.
    execute(admin, "default", batch).await;
    let mut batch = Batch::new();
    batch.id("setup-repo");
    batch.create_repo(
        "repo",
        ddl::create_repo(REPO).engine("fjall").tables(TABLES),
    );
    execute(admin, DB, batch).await;
    // Open (0o777) db + repo + each table so the replicator-role session
    // gains Read via the normal Shomer DAC path (repl_pull_e2e pattern).
    open(admin, DB, None, None).await;
    open(admin, DB, Some(REPO), None).await;
    for table in TABLES {
        open(admin, DB, Some(REPO), Some(table)).await;
    }

    admin
        .create_scram_user(
            child::REPLICATOR,
            Zeroizing::new(child::REPLICATOR_PW.to_string()),
            vec![],
        )
        .await
        .expect("create replicator user");
    admin
        .set_replicator(child::REPLICATOR, true)
        .await
        .expect("grant replicator role");

    let mut batch = Batch::new();
    batch.id("pub");
    batch.op(
        "publication",
        ddl::publication(PUBLICATION)
            .scope(repl_scope(DB, REPO))
            .build(),
    );
    execute(admin, DB, batch).await;
}

fn repl_scope(db: &str, repo: &str) -> shamir_query_types::admin::ReplScope {
    ddl::repl_scope(db).repo(repo).build()
}

async fn open(admin: &Client, db: &str, repo: Option<&str>, table: Option<&str>) {
    let res = match (repo, table) {
        (Some(r), Some(t)) => ddl::res::table(db, r, t),
        (Some(r), None) => ddl::res::store(db, r),
        _ => ddl::res::database(db),
    };
    let key = shamir_query_types::hmac::derive_session_hmac_key(&admin.session_id());
    // The v2 canonical form is computed over the op itself (tag excluded).
    let unsigned = shamir_query_types::batch::BatchOp::Chmod(shamir_query_types::admin::ChmodOp {
        chmod: res.clone(),
        mode: 0o777,
        hmac: None,
    });
    let canonical =
        shamir_query_types::hmac::canonical_batch_op(db, &unsigned).expect("chmod canonical form");
    let tag = shamir_query_types::hmac::compute_tag_hex(&key, &canonical);
    let mut batch = Batch::new();
    batch.id("open");
    batch.chmod(
        "chmod",
        ddl::chmod(res, 0o777)
            .hmac(tag)
            .build()
            .expect("build signed chmod"),
    );
    execute(admin, db, batch).await;
}

async fn execute(admin: &Client, db: &str, batch: Batch) {
    let response = admin
        .execute(db, batch.build())
        .await
        .expect("leader execute");
    assert!(
        !response.results.contains_key("__error"),
        "leader batch failed: {response:?}",
    );
}

/// One transactional insert of key `id` with value `v`.
pub async fn put(admin: &Client, table: &str, id: &str, v: i64) {
    let mut batch = Batch::new();
    batch.id("put");
    batch.transactional();
    batch.insert(
        "put",
        write::Insert::with_repo(REPO, table).rows([doc! {
            "id" => id,
            "v" => v,
        }]),
    );
    execute(admin, DB, batch).await;
}

/// Typed update of key `id` to value `v`.
pub async fn update_row(admin: &Client, table: &str, id: &str, v: i64) {
    let mut batch = Batch::new();
    batch.id("upd");
    batch.transactional();
    batch
        .try_update(
            "upd",
            write::Update::with_repo(REPO, table)
                .where_(eq("id", id))
                .set(doc! {
                    "id" => id,
                    "v" => v,
                }),
        )
        .expect("update op builds");
    execute(admin, DB, batch).await;
}

/// Typed delete of key `id`.
pub async fn delete_row(admin: &Client, table: &str, id: &str) {
    let mut batch = Batch::new();
    batch.id("del");
    batch.transactional();
    batch
        .try_delete(
            "del",
            write::Delete::with_repo(REPO, table).where_(eq("id", id)),
        )
        .expect("delete op builds");
    execute(admin, DB, batch).await;
}

/// Full row set of `table`, sorted by `id` — the convergence comparator.
pub async fn read_rows(client: &Client, table: &str) -> Vec<QueryValue> {
    let mut batch = Batch::new();
    batch.id("read");
    batch.query("rows", Query::with_repo(REPO, table));
    let response = client
        .execute(DB, batch.build())
        .await
        .expect("leader read");
    let result = &response.results["rows"];
    assert!(
        !response.results.contains_key("__error"),
        "leader read failed: {response:?}",
    );
    assert!(!result.skipped, "leader read skipped: {result:?}");
    let mut rows: Vec<QueryValue> = result
        .records
        .iter()
        .map(|record| record.as_value().into_owned())
        .collect();
    sort_rows(&mut rows);
    rows
}

pub fn sort_rows(rows: &mut [QueryValue]) {
    rows.sort_by_key(|row| {
        row.get("id")
            .and_then(QueryValue::as_str)
            .unwrap_or_default()
            .to_string()
    });
}

/// Hello snapshot of the app/main gap certificate. Uses a replicator
/// session — the same fence input the follower loop sees.
pub async fn gap_certificate(addr: SocketAddr) -> GapCert {
    let client = child::connect(addr, child::REPLICATOR, child::REPLICATOR_PW).await;
    let reply = client
        .repl(ReplRequest::Hello {
            proto_ver: 1,
            node_id: "seed-e2e-cert".to_string(),
            capabilities: None,
        })
        .await
        .expect("replicator hello");
    let ReplResponse::Hello { repos, .. } = reply else {
        panic!("expected Hello reply, got {reply:?}");
    };
    let repo = repos
        .iter()
        .find(|repo| repo.db == DB && repo.repo == REPO)
        .expect("hello advertises app/main");
    GapCert {
        lineage: repo.repo_lineage_id.expect("lineage advertised"),
        epoch: repo.history_epoch.expect("history epoch advertised"),
        gap_first: repo.gap_first,
        gap_through: repo.gap_through,
        schema: repo
            .schema_digest
            .clone()
            .expect("schema digest advertised"),
    }
}

/// The leader's current version for app/main (Hello `current_version`).
pub async fn current_version(client: &Client) -> u64 {
    let reply = client
        .repl(ReplRequest::Hello {
            proto_ver: 1,
            node_id: "seed-e2e-version".to_string(),
            capabilities: None,
        })
        .await
        .expect("hello for current_version");
    let ReplResponse::Hello { repos, .. } = reply else {
        panic!("expected Hello reply, got {reply:?}");
    };
    repos
        .iter()
        .find(|repo| repo.db == DB && repo.repo == REPO)
        .expect("hello advertises app/main")
        .current_version
}

/// Alias so scenarios can build synthetic mismatch Hellos of the same
/// shape the wire produces.
pub fn synthetic_hello(
    lineage: [u8; 16],
    epoch: u64,
    schema: &str,
    publications: Vec<CreatePublicationOp>,
    settled_version: u64,
) -> ReplResponse {
    ReplResponse::Hello {
        leader_epoch: 1,
        repos: vec![ReplRepoInfo {
            db: DB.to_string(),
            repo: REPO.to_string(),
            current_version: settled_version,
            journal_floor: 1,
            repo_lineage_id: Some(lineage),
            history_epoch: Some(epoch),
            gap_first: None,
            gap_through: None,
            schema_digest: Some(schema.to_string()),
            settled_version: Some(settled_version),
            publications,
        }],
    }
}
