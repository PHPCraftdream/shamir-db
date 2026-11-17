//! Index and interner metrics snapshot (`shamir_index_*`, `shamir_interner_*`).
//!
//! Scrape-time composition only: the build/rebuild histograms come from the
//! per-repo `EngineMetrics` (lock-free), the entry gauges are sampled live
//! from the existing per-family entry-count APIs, and the interner gauges
//! read `Interner::len()` + `InternerManager::persisted_high_water()`. No new
//! engine state is created here.
//!
//! Cardinality policy (design §4, const — not settings): at most
//! [`INDEX_TABLES_CAP`] tables and [`INDEX_INDEXES_CAP`] indexes per
//! repository carry per-table/per-index labels; the rest is aggregated into
//! ONE `entries_other` total (rendered by the exporter as a single
//! `table="(other)"` row per repository). Selection is deterministic:
//! descending current record count (`RecordCounter`), tie-break ascending
//! table name, then ascending index name.

use crate::repo::durability_policy::HistogramSnapshot;
use crate::repo::instance::repo_instance::RepoInstance;
use shamir_engine_table::index_metrics::IndexMetricKind;
use shamir_index::kind::IndexKind;
use shamir_storage::error::DbResult;
use shamir_types::core::interner::Interner;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::EngineMetrics;

/// Maximum tables per repository that carry per-table labels.
pub const INDEX_TABLES_CAP: usize = 64;

/// Maximum indexes per repository that carry per-index labels.
pub const INDEX_INDEXES_CAP: usize = 32;

/// Resolve an index's DDL label from its interned id; falls back to the raw
/// id string if the reverse lookup misses (should not happen for live defs).
fn index_label(interner: &Interner, name_interned: u64) -> String {
    interner
        .get_str(&interner.make_key(name_interned))
        .map(|s| s.to_string())
        .unwrap_or_else(|| name_interned.to_string())
}

/// One labelled `shamir_index_entries` sample row.
pub struct IndexEntryRow {
    pub table: String,
    pub index: String,
    pub kind: IndexMetricKind,
    pub entries: u64,
}

/// Cold-path copy of a repository's index + interner metrics.
pub struct IndexMetricsSnapshot {
    /// Build histograms by [`IndexMetricKind::index`].
    pub build: [HistogramSnapshot; 3],
    /// Rebuild histograms by [`IndexMetricKind::index`].
    pub rebuild: [HistogramSnapshot; 3],
    /// Labelled entry-gauge rows (within the caps).
    pub entries: Vec<IndexEntryRow>,
    /// Summed entries of every row beyond the caps; the exporter renders this
    /// as the single `table="(other)"` gauge row per repository.
    pub entries_other: u64,
    /// `shamir_interner_names` — `Interner::len()`.
    pub interner_names: u64,
    /// `shamir_interner_high_water` — `InternerManager::persisted_high_water()`.
    pub interner_high_water: u64,
    /// `shamir_export_tables_known`.
    pub tables_known: u64,
    /// `shamir_export_tables_truncated` (1/0) — the table cap applied.
    pub tables_truncated: bool,
}

/// Scrape the index/interner snapshot for one repository.
///
/// The body walks every open table, so the future is heap-boxed here (one
/// allocation per scrape) to keep it off caller stacks — the Windows
/// debug-build stack-overflow precedent for large awaited futures.
pub fn index_metrics_snapshot<'a>(
    repo: &'a RepoInstance,
) -> Pin<Box<dyn Future<Output = DbResult<IndexMetricsSnapshot>> + Send + 'a>> {
    Box::pin(collect_index_interner_snapshot(repo))
}

/// Wire the repository's metrics bundle into a table manager so its index
/// builds/rebuilds are observed. Idempotent (first installation wins); call
/// on any `TableManager` handle — the slot is shared across clones.
pub fn wire_table_index_metrics(
    table: &shamir_engine_table::TableManager,
    metrics: &Arc<EngineMetrics>,
) -> bool {
    let observer: Arc<dyn shamir_engine_table::index_metrics::IndexBuildObserver> = metrics.clone();
    table.set_index_build_observer(observer)
}

async fn collect_index_interner_snapshot(repo: &RepoInstance) -> DbResult<IndexMetricsSnapshot> {
    let (build, rebuild) = repo.query_metrics().index_metric_parts();

    // Live sampling: every open table, every family's own enumeration and
    // entry-count API. Open-registry only (closed tables are invisible), the
    // same documented limitation as the gate/degraded-index exporters.
    let mut table_names = repo.list_table_names();
    table_names.sort();
    let tables_known = table_names.len() as u64;
    let tables_truncated = table_names.len() > INDEX_TABLES_CAP;

    // (record_count, table_name, index_name, kind, entries) rows; selection
    // order: record count DESC, table name ASC, index name ASC.
    let mut rows: Vec<(u64, String, String, IndexMetricKind, u64)> = Vec::new();
    for name in &table_names {
        let table = repo.get_table(name).await?;
        let count = table.count().await? as u64;
        let interner = table.interner().get().await?;
        let index_manager = table.index_manager_ref();
        for def in index_manager.iter_indexes() {
            let label = index_label(interner, def.name_interned);
            let entries = index_manager.entry_count(def.name_interned, false).await?;
            rows.push((
                count,
                name.clone(),
                label,
                IndexMetricKind::Regular,
                entries,
            ));
        }
        for def in index_manager.iter_unique_indexes() {
            let label = index_label(interner, def.name_interned);
            let entries = index_manager.entry_count(def.name_interned, true).await?;
            rows.push((
                count,
                name.clone(),
                label,
                IndexMetricKind::Regular,
                entries,
            ));
        }
        for def in table.sorted_indexes().iter_indexes() {
            let label = index_label(interner, def.name_interned);
            let entries = table
                .sorted_indexes()
                .entry_count(def.name_interned)
                .await?;
            rows.push((count, name.clone(), label, IndexMetricKind::Sorted, entries));
        }
        for backend in table.index2_registry().all_backends().await {
            let descriptor = backend.descriptor();
            // Only FTS backends are in the kind set; functional/vector
            // indexes have no entry-gauge row in this step.
            if !matches!(descriptor.kind, IndexKind::Fts { .. }) {
                continue;
            }
            let entries = backend
                .as_any()
                .downcast_ref::<shamir_index::fts_ranked_backend::FtsRankedBackend>()
                .map(|fts| fts.doc_count())
                .unwrap_or(0);
            rows.push((
                count,
                name.clone(),
                descriptor.name.clone(),
                IndexMetricKind::Fts,
                entries,
            ));
        }
    }

    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let mut entries = Vec::new();
    let mut entries_other = 0u64;
    for (position, (_, table, index, kind, entry_count)) in rows.into_iter().enumerate() {
        if position < INDEX_INDEXES_CAP {
            entries.push(IndexEntryRow {
                table,
                index,
                kind,
                entries: entry_count,
            });
        } else {
            entries_other += entry_count;
        }
    }

    // Interner gauges: map-length read + the persisted high-water mark; both
    // scrape-time only.
    let interner_manager = repo.repo_interner().await?;
    let interner_names = interner_manager.get().await?.len() as u64;
    let interner_high_water = interner_manager.persisted_high_water() as u64;

    Ok(IndexMetricsSnapshot {
        build,
        rebuild,
        entries,
        entries_other,
        interner_names,
        interner_high_water,
        tables_known,
        tables_truncated,
    })
}
