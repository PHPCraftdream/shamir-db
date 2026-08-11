//! `doctor` CLI command — verify and repair table integrity.
//!
//! Offline only: opens `data_dir` directly via [`ShamirDb::init`], scans
//! tables (or a filtered subset via `--db`/`--repo`/`--table`), and reports
//! health diagnostics. With `--apply`, repairs unhealthy tables by dropping
//! and rebuilding all indexes.
//!
//! Exits with non-zero if any table is unhealthy (useful for CI/health checks).

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use shamir_collections::TFxMap;
use shamir_db::engine;
use shamir_db::engine::index2::IndexState;
use shamir_db::engine::table::doctor::{Index2Health, IndexHealth, RepairReport, VerifyReport};
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;

use crate::config::Config;
use crate::tables_registry::TablesRegistry;

/// Wrapper for `IndexHealth` that includes the resolved index name
/// for JSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NamedIndexHealth {
    /// Raw name_interned value.
    #[serde(rename = "name_interned")]
    pub name_interned: u64,
    /// Expected entry count.
    pub expected_entries: u64,
    /// Actual entry count.
    pub actual_entries: u64,
    /// Index state.
    pub state: IndexState,
    /// Optional diagnostic message.
    pub message: Option<String>,
    /// Resolved human-readable index name (from interner).
    pub resolved_name: String,
}

impl NamedIndexHealth {
    fn from_health(health: &IndexHealth, resolved_name: String) -> Self {
        Self {
            name_interned: health.name_interned,
            expected_entries: health.expected_entries,
            actual_entries: health.actual_entries,
            state: health.state,
            message: health.message.clone(),
            resolved_name,
        }
    }
}

/// Verify report with resolved index names for JSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerifyReportJson {
    pub records_in_data: u64,
    pub counter_value: u64,
    pub counter_consistent: bool,
    pub regular_indexes: Vec<NamedIndexHealth>,
    pub unique_indexes: Vec<NamedIndexHealth>,
    pub sorted_indexes: Vec<NamedIndexHealth>,
    pub index2_backends: Vec<Index2Health>,
    pub index2_registry_consistency: Vec<String>,
    pub cross_family_name_collisions: Vec<String>,
}

impl VerifyReportJson {
    fn from_report(report: &VerifyReport, name_map: &TFxMap<u64, String>) -> Self {
        let resolve = |idx: &IndexHealth| {
            let name = name_map
                .get(&idx.name_interned)
                .cloned()
                .unwrap_or_else(|| format!("<name_interned={}>", idx.name_interned));
            NamedIndexHealth::from_health(idx, name)
        };

        Self {
            records_in_data: report.records_in_data,
            counter_value: report.counter_value,
            counter_consistent: report.counter_consistent,
            regular_indexes: report.regular_indexes.iter().map(resolve).collect(),
            unique_indexes: report.unique_indexes.iter().map(resolve).collect(),
            sorted_indexes: report.sorted_indexes.iter().map(resolve).collect(),
            index2_backends: report.index2_backends.clone(),
            index2_registry_consistency: report.index2_registry_consistency.clone(),
            cross_family_name_collisions: report.cross_family_name_collisions.clone(),
        }
    }
}

/// Parsed `doctor` arguments (mirrors the clap subcommand in `main`).
#[derive(Debug, Clone, Default)]
pub struct DoctorArgs {
    /// Restrict to a single database (default: all databases).
    pub db: Option<String>,
    /// Restrict to a single repository (default: all repositories).
    pub repo: Option<String>,
    /// Restrict to a single table (default: all tables).
    pub table: Option<String>,
    /// Apply repairs to unhealthy tables (default: read-only verify).
    pub apply: bool,
    /// Emit pretty-printed JSON output.
    pub pretty: bool,
    /// Emit machine-readable JSON output.
    pub json: bool,
}

/// Combined report for a single table: verify results, optional repair results.
/// (Internal struct, not directly serialized — see `TableReportJson` for JSON.)
#[derive(Debug, Clone)]
pub struct TableReport {
    /// Database name.
    pub db: String,
    /// Repository name.
    pub repo: String,
    /// Table name.
    pub table: String,
    /// Verify report (always present).
    pub verify: VerifyReport,
    /// Repair report (present only if `--apply` was used and table was unhealthy).
    pub repair: Option<RepairReport>,
    /// Whether the table is healthy after this run.
    pub healthy: bool,
    /// Resolved index names for this table (`name_interned` → human-readable).
    /// Embedded so the report is self-contained for JSON conversion and tests.
    pub resolved_index_names: TFxMap<u64, String>,
}

/// JSON-serializable table report with resolved index names.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TableReportJson {
    pub db: String,
    pub repo: String,
    pub table: String,
    pub verify: VerifyReportJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair: Option<RepairReport>,
    pub healthy: bool,
}

impl TableReportJson {
    fn from_report(report: &TableReport) -> Self {
        let name_map = &report.resolved_index_names;
        Self {
            db: report.db.clone(),
            repo: report.repo.clone(),
            table: report.table.clone(),
            verify: VerifyReportJson::from_report(&report.verify, name_map),
            repair: report.repair.clone(),
            healthy: report.healthy,
        }
    }
}

/// Aggregate report for all tables scanned. (Internal struct, see
/// `DoctorReportJson` for JSON.)
#[derive(Debug, Clone)]
pub struct DoctorReport {
    /// Per-table reports.
    pub tables: Vec<TableReport>,
    /// Overall health (true if every table is healthy).
    pub healthy: bool,
    /// Total tables scanned.
    pub total_tables: usize,
    /// Total unhealthy tables (before repair, if `--apply` was used).
    pub unhealthy_before: usize,
    /// Total tables repaired (if `--apply` was used).
    pub repaired: usize,
}

impl DoctorReport {
    /// Serialize to a JSON string with resolved index names.
    /// Exposed so tests can validate the JSON structure without capturing
    /// stdout (no established stdout-capture pattern exists in this crate).
    pub fn to_json(&self, pretty: bool) -> anyhow::Result<String> {
        let json_report = DoctorReportJson {
            tables: self
                .tables
                .iter()
                .map(TableReportJson::from_report)
                .collect(),
            healthy: self.healthy,
            total_tables: self.total_tables,
            unhealthy_before: self.unhealthy_before,
            repaired: self.repaired,
        };
        if pretty {
            Ok(serde_json::to_string_pretty(&json_report)?)
        } else {
            Ok(serde_json::to_string(&json_report)?)
        }
    }
}

/// JSON-serializable aggregate report with resolved index names.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DoctorReportJson {
    pub tables: Vec<TableReportJson>,
    pub healthy: bool,
    pub total_tables: usize,
    pub unhealthy_before: usize,
    pub repaired: usize,
}

/// Run the command: scan tables (or filtered subset), verify (and optionally
/// repair), print report, and return the aggregate [`DoctorReport`].
///
/// Returns `Err` (non-zero exit via `main`'s `Termination`) when any table is
/// unhealthy or when an explicit `--db`/`--repo`/`--table` filter matches
/// zero tables — the latter is fail-loud because the operator asked for a
/// specific table and got silence.
pub async fn run(config: &Config, args: &DoctorArgs) -> anyhow::Result<DoctorReport> {
    let meta_path = config.data_dir.join("shamir_db_meta.redb");

    // redb is single-writer: retry briefly before failing with a clear
    // "is the server stopped?" error (mirrors access_tree.rs pattern).
    let shamir = {
        let mut last_err = None;
        let mut opened = None;
        for _ in 0..20 {
            match ShamirDb::init(SystemStoreConfig::Fjall(meta_path.clone())).await {
                Ok(db) => {
                    opened = Some(db);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
        match opened {
            Some(db) => db,
            None => {
                return Err(anyhow!(
                    "open data_dir (is the server stopped?): {}",
                    last_err.expect("at least one attempt failed")
                ))
            }
        }
    };

    // Replay the wire-tables registry: re-register every table that a wire
    // client created in a previous boot so its data file is re-attached to
    // the in-memory `RepoInstance`. Without this step the table exists on
    // disk but doctor cannot see it (mirrors server_launcher.rs boot path).
    let tables_registry =
        TablesRegistry::open(&config.data_dir).context("open wire_tables registry")?;
    {
        let snap = tables_registry.snapshot();
        for (db_name, repo_name, table_name) in snap.iter_entries() {
            if let Some(db) = shamir.get_db(db_name) {
                if !db.has_table(repo_name, table_name) {
                    if let Err(e) = db.create_table(repo_name, table_name) {
                        tracing::warn!(
                            db = db_name,
                            repo = repo_name,
                            table = table_name,
                            ?e,
                            "tables_registry replay: create_table failed"
                        );
                    }
                }
            }
        }
    }

    // Collect all matching tables.
    let mut table_reports: Vec<TableReport> = Vec::new();

    for db_name in shamir.list_dbs() {
        if let Some(ref filter_db) = args.db {
            if &db_name != filter_db {
                continue;
            }
        }

        let Some(db) = shamir.get_db(&db_name) else {
            continue;
        };

        for repo_name in db.list_repos() {
            if let Some(ref filter_repo) = args.repo {
                if &repo_name != filter_repo {
                    continue;
                }
            }

            let Some(repo) = db.get_repo(&repo_name) else {
                continue;
            };

            for table_name in repo.list_table_names() {
                if let Some(ref filter_table) = args.table {
                    if &table_name != filter_table {
                        continue;
                    }
                }

                // Get the table manager (lazy-open).
                let table_mgr = repo.get_table(&table_name).await.with_context(|| {
                    format!("open table '{}.{}.{}'", db_name, repo_name, table_name)
                })?;

                // Verify the table.
                let verify_report = table_mgr.verify().await.with_context(|| {
                    format!("verify table '{}.{}.{}'", db_name, repo_name, table_name)
                })?;

                // Resolve index names for human-readable output.
                let index_name_map = resolve_index_names(&table_mgr, &verify_report).await;

                let is_healthy = verify_report.is_healthy();
                let repair_report = if args.apply && !is_healthy {
                    // Repair the table.
                    let repair = table_mgr.repair().await.with_context(|| {
                        format!("repair table '{}.{}.{}'", db_name, repo_name, table_name)
                    })?;
                    Some(repair)
                } else {
                    None
                };

                let final_healthy = if args.apply && !is_healthy {
                    // After repair, verify again to confirm health.
                    table_mgr
                        .verify()
                        .await
                        .with_context(|| {
                            format!(
                                "post-repair verify table '{}.{}.{}'",
                                db_name, repo_name, table_name
                            )
                        })?
                        .is_healthy()
                } else {
                    is_healthy
                };

                table_reports.push(TableReport {
                    db: db_name.clone(),
                    repo: repo_name.clone(),
                    table: table_name.clone(),
                    verify: verify_report,
                    repair: repair_report,
                    healthy: final_healthy,
                    resolved_index_names: index_name_map,
                });
            }
        }
    }

    // No tables matched. An explicit filter matching nothing is an error
    // (the operator asked for a specific table and got silence); a
    // genuinely empty database with no filter is not.
    if table_reports.is_empty() {
        if args.db.is_some() || args.repo.is_some() || args.table.is_some() {
            eprintln!("No tables match the given filters.");
            return Err(anyhow!(
                "doctor: no tables match the given filters (db={:?}, repo={:?}, table={:?})",
                args.db,
                args.repo,
                args.table
            ));
        }
        eprintln!("No tables found in the database.");
        return Ok(DoctorReport {
            tables: Vec::new(),
            healthy: true,
            total_tables: 0,
            unhealthy_before: 0,
            repaired: 0,
        });
    }

    // Build aggregate report.
    let unhealthy_before = table_reports
        .iter()
        .filter(|r| !r.verify.is_healthy())
        .count();
    let repaired = table_reports.iter().filter(|r| r.repair.is_some()).count();
    let healthy = table_reports.iter().all(|r| r.healthy);

    let total_tables = table_reports.len();
    let report = DoctorReport {
        tables: table_reports,
        healthy,
        total_tables,
        unhealthy_before,
        repaired,
    };

    // Output.
    if args.json || args.pretty {
        let json = report.to_json(args.pretty)?;
        println!("{}", json);
    } else {
        // Human-readable text output.
        print_human_report(&report);
    }

    // Propagate as `Err` so `main`'s `Termination` handler runs destructors
    // (Fjall/redb handles, background flush tasks) before the process exits —
    // unlike `std::process::exit` which skips them entirely.
    if !healthy {
        return Err(anyhow!(
            "doctor: {unhealthy_before} of {total_tables} table(s) unhealthy"
        ));
    }

    Ok(report)
}

/// Resolve index names from `name_interned` values to human-readable strings
/// using the table's interner.
async fn resolve_index_names(
    table_mgr: &engine::table::TableManager,
    verify_report: &VerifyReport,
) -> TFxMap<u64, String> {
    let mut name_map: TFxMap<u64, String> = TFxMap::default();

    // Collect all name_interned values.
    for idx in &verify_report.regular_indexes {
        name_map
            .entry(idx.name_interned)
            .or_insert_with(|| format!("<name_interned={}>", idx.name_interned));
    }
    for idx in &verify_report.unique_indexes {
        name_map
            .entry(idx.name_interned)
            .or_insert_with(|| format!("<name_interned={}>", idx.name_interned));
    }
    for idx in &verify_report.sorted_indexes {
        name_map
            .entry(idx.name_interned)
            .or_insert_with(|| format!("<name_interned={}>", idx.name_interned));
    }

    // Resolve via interner (mirrors access_tree.rs pattern).
    if let Ok(interner) = table_mgr.interner().get().await {
        for (name_interned, name) in name_map.iter_mut() {
            let key = shamir_types::core::interner::InternerKey::new(*name_interned);
            if let Some(s) = interner.get_str(&key) {
                *name = (*s).to_string();
            }
        }
    }

    name_map
}

/// Print a human-readable text report.
fn print_human_report(report: &DoctorReport) {
    println!("Doctor Report — {} table(s) scanned", report.total_tables);
    println!();

    for table_report in &report.tables {
        let name_map = &table_report.resolved_index_names;
        println!(
            "{}.{}.{}:",
            table_report.db, table_report.repo, table_report.table
        );

        // Counter consistency.
        if table_report.verify.counter_consistent {
            println!(
                "  ✓ counter: consistent ({}/{} records)",
                table_report.verify.counter_value, table_report.verify.records_in_data
            );
        } else {
            println!(
                "  ✗ counter: INCONSISTENT (value={}, expected={})",
                table_report.verify.counter_value, table_report.verify.records_in_data
            );
        }

        // Regular indexes.
        if !table_report.verify.regular_indexes.is_empty() {
            println!("  regular indexes:");
            for idx in &table_report.verify.regular_indexes {
                print_index_health(idx, name_map.get(&idx.name_interned).map(String::as_str));
            }
        }

        // Unique indexes.
        if !table_report.verify.unique_indexes.is_empty() {
            println!("  unique indexes:");
            for idx in &table_report.verify.unique_indexes {
                print_index_health(idx, name_map.get(&idx.name_interned).map(String::as_str));
            }
        }

        // Sorted indexes.
        if !table_report.verify.sorted_indexes.is_empty() {
            println!("  sorted indexes:");
            for idx in &table_report.verify.sorted_indexes {
                print_index_health(idx, name_map.get(&idx.name_interned).map(String::as_str));
            }
        }

        // Index2 backends.
        if !table_report.verify.index2_backends.is_empty() {
            println!("  index2 backends:");
            for idx in &table_report.verify.index2_backends {
                print_index2_health(idx);
            }
        }

        // Index2 registry consistency.
        if !table_report.verify.index2_registry_consistency.is_empty() {
            println!("  index2 registry consistency:");
            for problem in &table_report.verify.index2_registry_consistency {
                println!("    ✗ {}", problem);
            }
        }

        // Cross-family name collisions.
        if !table_report.verify.cross_family_name_collisions.is_empty() {
            println!("  cross-family name collisions:");
            for collision in &table_report.verify.cross_family_name_collisions {
                println!("    ✗ {}", collision);
            }
        }

        // Repair report (if any).
        if let Some(ref repair) = table_report.repair {
            println!("  repaired:");
            println!("    records scanned: {}", repair.records_scanned);
            println!(
                "    counter: {} → {}",
                repair.counter_before, repair.counter_after
            );
            println!(
                "    regular indexes rebuilt: {}",
                repair.regular_indexes_rebuilt
            );
            println!(
                "    unique indexes rebuilt: {}",
                repair.unique_indexes_rebuilt
            );
            println!(
                "    sorted indexes rebuilt: {}",
                repair.sorted_indexes_rebuilt
            );
            println!("    elapsed: {}ms", repair.elapsed_ms);
        }

        // Overall health.
        if table_report.healthy {
            println!("  ✓ healthy");
        } else {
            println!("  ✗ UNHEALTHY");
        }

        println!();
    }

    // Summary.
    println!("Summary:");
    println!("  total tables: {}", report.total_tables);
    println!("  unhealthy (before repair): {}", report.unhealthy_before);
    if report.repaired > 0 {
        println!("  repaired: {}", report.repaired);
    }
    if report.healthy {
        println!("  overall: ✓ all tables healthy");
    } else {
        println!("  overall: ✗ some tables unhealthy");
    }
}

/// Print health status for a base index (regular/unique/sorted).
fn print_index_health(idx: &IndexHealth, resolved_name: Option<&str>) {
    let fallback = format!("<name_interned={}>", idx.name_interned);
    let name_display = resolved_name.unwrap_or(&fallback);

    if idx.is_healthy() {
        println!(
            "    ✓ index '{}': {} entries, Ready",
            name_display, idx.actual_entries
        );
    } else {
        let state_str = match idx.state {
            IndexState::Ready => "Ready",
            IndexState::Building => "Building",
            IndexState::Failed => "Failed",
        };
        println!(
            "    ✗ index '{}': expected={}, actual={}, state={}",
            name_display, idx.expected_entries, idx.actual_entries, state_str
        );
        if let Some(ref msg) = idx.message {
            println!("      message: {}", msg);
        }
    }
}

/// Print health status for an index2 backend.
fn print_index2_health(idx: &Index2Health) {
    if idx.healthy {
        println!("    ✓ index2 '{}': Ready", idx.name);
    } else {
        let state_str = match idx.state {
            IndexState::Ready => "Ready",
            IndexState::Building => "Building",
            IndexState::Failed => "Failed",
        };
        println!("    ✗ index2 '{}' (id={}): {}", idx.name, idx.id, state_str);
        if let Some(ref msg) = idx.message {
            println!("      message: {}", msg);
        }
    }
}
