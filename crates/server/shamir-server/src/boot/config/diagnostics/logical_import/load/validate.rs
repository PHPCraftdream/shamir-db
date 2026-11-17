//! Post-load validation: re-read the staged content through the engine read
//! facade (same MaintenanceGate freeze and system-repo capture as the export
//! driver) and compare it against the artifact inventory: counts per table,
//! key sets, value equality, settings, and ownership resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use sha2::{Digest, Sha256};
use shamir_db::engine::db_instance::logical_export::{LogicalExport, MaintenanceGate};
use shamir_db::{DbError, ShamirDb};
use shamir_sync::replication_seed::{
    encode_canonical, from_query_value, DumpReader, LogicalValue, PortableItem, RepositoryItem,
    SettingKey,
};
use shamir_types::types::value::QueryValue;
use tokio_util::sync::CancellationToken;

use super::super::error::LogicalImportError;
use super::super::reader::CancelledRead;
use super::preflight::Preflight;

/// Validate the staged database against the artifact. `home` stays open;
/// the freeze admission is attempt-owned like the export driver's.
pub(crate) async fn run(
    home: &ShamirDb,
    pf: &Preflight,
    db_name: &str,
    owner_ids: &BTreeMap<String, u64>,
    artifact: &Path,
    cancel: &CancellationToken,
) -> Result<(), LogicalImportError> {
    if cancel.is_cancelled() {
        return Err(LogicalImportError::Cancelled);
    }
    let owned = home.clone();
    let expected = Expected::new(pf, db_name, owner_ids);
    let artifact = artifact.to_owned();
    let cancel = cancel.clone();
    let gate = MaintenanceGate::default();
    gate.with_freeze(move |guard| {
        Box::pin(async move {
            let work = async {
                let db = owned
                    .get_db(&expected.db_name)
                    .ok_or_else(|| mismatch("database did not load".into()))?;
                owned.check_seed_catalogue().await?;
                let reader = owned
                    .system_store()
                    .capture_logical_export(&db, guard)
                    .await?;
                check_definitions(&reader, &expected).await?;
                check_records(&reader, &expected, &artifact, &cancel).await?;
                Ok(())
            }
            .await;
            work.map_err(|e: LogicalImportError| match e {
                LogicalImportError::Validation(message) => {
                    DbError::Validation(format!("logical-import validation: {message}"))
                }
                other => DbError::Validation(format!("{other:#}")),
            })
        })
    })
    .await?;
    Ok(())
}

#[derive(Clone)]
struct Expected {
    db_name: String,
    database_owner: Option<u64>,
    repos: BTreeMap<String, RepositoryItem>,
    tables: BTreeMap<String, TableExpectation>,
    owners: BTreeMap<String, u64>,
    root_meta_owner: Option<u64>,
}

#[derive(Clone)]
struct TableExpectation {
    enable_indexes: bool,
    ttl_ms: Option<u64>,
}

impl Expected {
    fn new(pf: &Preflight, db_name: &str, owner_ids: &BTreeMap<String, u64>) -> Self {
        let owners = owner_ids.clone();
        let owner_of =
            |name: &Option<String>| name.as_deref().and_then(|name| owners.get(name).copied());
        let tables = pf
            .tables
            .values()
            .map(|item| {
                (
                    format!("{}/{}", item.repository, item.name),
                    TableExpectation {
                        enable_indexes: item.enable_indexes,
                        ttl_ms: item.buffer_ttl_ms,
                    },
                )
            })
            .collect();
        let root_meta_owner = pf
            .settings
            .iter()
            .find(|item| item.key == SettingKey::RootMeta)
            .and_then(|item| match &item.value {
                LogicalValue::Map(fields) => fields.iter().find(|(k, _)| k == "owner"),
                _ => None,
            })
            .and_then(|(_, value)| match value {
                LogicalValue::Str(name) => owners.get(name).copied(),
                _ => None,
            });
        Self {
            db_name: db_name.to_owned(),
            database_owner: owner_of(&pf.database.owner),
            repos: pf.repos.clone(),
            tables,
            owners,
            root_meta_owner,
        }
    }

    fn owner(&self, name: Option<&str>) -> Option<u64> {
        name.and_then(|name| self.owners.get(name).copied())
    }
}

fn mismatch(message: String) -> LogicalImportError {
    LogicalImportError::Validation(message)
}

/// Numeric owner the catalogue stores for the artifact's logical owner name.
fn row_owner(row: &QueryValue) -> Option<u64> {
    match row.get("owner") {
        Some(QueryValue::Int(id)) if *id > 0 => u64::try_from(*id).ok(),
        _ => None,
    }
}

fn text<'a>(row: &'a QueryValue, key: &str) -> Result<&'a str, LogicalImportError> {
    row.get(key)
        .and_then(QueryValue::as_str)
        .ok_or_else(|| mismatch(format!("catalogue row is missing '{key}'")))
}

async fn check_definitions(
    reader: &LogicalExport<'_>,
    expected: &Expected,
) -> Result<(), LogicalImportError> {
    let mut seen_repos = BTreeSet::new();
    let mut seen_tables = BTreeSet::new();
    for family in ["databases", "repositories", "tables", "settings"] {
        let mut rows = reader.catalogue_rows(family).await?;
        while let Some(page) = rows.next_page().await? {
            for (_, row) in page {
                match family {
                    "databases" => {
                        if text(&row, "name")? != expected.db_name {
                            continue;
                        }
                        if row_owner(&row) != expected.database_owner {
                            return Err(mismatch(
                                "database ownership does not match the artifact".into(),
                            ));
                        }
                    }
                    "repositories" => {
                        let name = text(&row, "repo_name")?.to_owned();
                        let item = expected.repos.get(&name).ok_or_else(|| {
                            mismatch(format!("staged repository '{name}' is not in the artifact"))
                        })?;
                        if row_owner(&row) != expected.owner(item.owner.as_deref()) {
                            return Err(mismatch(format!(
                                "repository '{name}' ownership does not match the artifact"
                            )));
                        }
                        seen_repos.insert(name);
                    }
                    "tables" => {
                        let repo = text(&row, "repo_name")?.to_owned();
                        let name = text(&row, "table_name")?.to_owned();
                        let key = format!("{repo}/{name}");
                        let item = expected.tables.get(&key).ok_or_else(|| {
                            mismatch(format!("staged table '{key}' is not in the artifact"))
                        })?;
                        match row.get("enable_indexes") {
                            Some(QueryValue::Bool(actual)) if *actual == item.enable_indexes => {}
                            _ => {
                                return Err(mismatch(format!(
                                    "table '{key}' enable_indexes does not match the artifact"
                                )))
                            }
                        }
                        let view = reader.table(&repo, &name)?;
                        let buffer = view.buffer_config().await?;
                        if buffer.as_ref().and_then(|b| b.ttl_ms) != item.ttl_ms {
                            return Err(mismatch(format!(
                                "table '{key}' buffer TTL does not match the artifact"
                            )));
                        }
                        seen_tables.insert(key);
                    }
                    _ => {
                        if text(&row, "key")? == "root_meta"
                            && row_owner(&row) != expected.root_meta_owner
                        {
                            return Err(mismatch(
                                "root_meta setting ownership does not match the artifact".into(),
                            ));
                        }
                    }
                }
            }
        }
    }
    if seen_repos.len() != expected.repos.len()
        || seen_tables.len() != expected.tables.len()
        || !expected.repos.keys().all(|repo| seen_repos.contains(repo))
        || !expected
            .tables
            .keys()
            .all(|table| seen_tables.contains(table))
    {
        return Err(mismatch(
            "staged repository/table inventory differs from the artifact".into(),
        ));
    }
    Ok(())
}

/// Streaming comparison of staged records against the artifact. Each side is
/// reduced to a per-table count and a commutative digest (sum of per-record
/// hashes over raw key + canonical value), so no whole table is held in
/// memory and record order does not matter.
async fn check_records(
    reader: &LogicalExport<'_>,
    expected: &Expected,
    artifact: &Path,
    cancel: &CancellationToken,
) -> Result<(), LogicalImportError> {
    let mut staged: BTreeMap<String, RecordSummary> = BTreeMap::new();
    for key in expected.tables.keys() {
        let (repo, table) = key.split_once('/').expect("table key shape");
        let mut rows = reader.records(repo, table).await?;
        while let Some(page) = rows.next_page().await? {
            for (raw_key, value) in page {
                let logical = from_query_value(&value)
                    .map_err(|e| mismatch(format!("staged value cannot be re-encoded: {e}")))?;
                let summary = staged.entry(key.clone()).or_default();
                summary.count += 1;
                summary.digest = summary
                    .digest
                    .wrapping_add(record_digest(raw_key.as_ref(), &logical)?);
            }
        }
    }
    let artifact_side = artifact_summary(artifact, cancel)?;
    for (table, summary) in &artifact_side {
        let observed = staged.get(table).cloned().unwrap_or_default();
        if observed.count != summary.count || observed.digest != summary.digest {
            return Err(mismatch(format!(
                "records of '{table}' differ from the artifact"
            )));
        }
    }
    if staged.len() != artifact_side.len() {
        return Err(mismatch(
            "staged record inventory differs from the artifact".into(),
        ));
    }
    Ok(())
}

#[derive(Default, Clone)]
struct RecordSummary {
    count: u64,
    digest: u64,
}

fn record_digest(key: &[u8], value: &LogicalValue) -> Result<u64, LogicalImportError> {
    let canonical = encode_canonical(value).map_err(|e| mismatch(format!("{e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(&canonical);
    let digest: [u8; 8] = hasher.finalize()[..8]
        .try_into()
        .expect("sha256 yields at least 8 bytes");
    Ok(u64::from_le_bytes(digest))
}

fn artifact_summary(
    artifact: &Path,
    cancel: &CancellationToken,
) -> Result<BTreeMap<String, RecordSummary>, LogicalImportError> {
    let file = std::fs::File::open(artifact)?;
    let mut reader = DumpReader::new(CancelledRead::new(file, cancel.clone()))
        .map_err(LogicalImportError::Artifact)?;
    let mut summaries: BTreeMap<String, RecordSummary> = BTreeMap::new();
    while let Some(item) = reader.next_item().map_err(LogicalImportError::Artifact)? {
        let PortableItem::Record(record) = item else {
            continue;
        };
        let summary = summaries
            .entry(format!("{}/{}", record.repository, record.table))
            .or_default();
        summary.count += 1;
        summary.digest = summary
            .digest
            .wrapping_add(record_digest(&record.raw_key, &record.value)?);
    }
    Ok(summaries)
}
