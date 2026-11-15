//! Query-layer home of the shared FK reverse-cache rebuild scan
//! (ENGINE-LAYERING R2d). The scan takes `&dyn BatchResolver` — a
//! query-layer trait — so it lives beside its resolver; the cache TYPES it
//! fills (`FkReverseCache`, `ReverseFkEntry`, `TaggedReverseFkEntry`) stay
//! in the table layer, and `crate::repo::fk_reverse_cache` re-exports this
//! fn so every historical `crate::repo::<...>::build_reverse_fk_entries`
//! path keeps resolving (instance -> query is the allowed direction).

use crate::query::batch::BatchResolver;
use shamir_query_types::TableRef;

use crate::table::fk_reverse_cache::{ReverseFkEntry, TaggedReverseFkEntry};

/// Shared O(tables) rebuild scan: for every table in `repo_name`, resolve it
/// and collect every FK it declares (ANY `on_delete` action, including
/// `NoAction` — the cache is a repo-wide snapshot both `fk_restrict.rs`'s
/// RESTRICT-only discovery and `fk_actions.rs`'s CASCADE/SET-NULL discovery
/// filter from, so it must carry every action, not just the ones one caller
/// happens to care about).
///
/// This is the exact scan `discover_restrict_refs`/`discover_action_refs`
/// used to run on every call before F-28 Step 4; now it runs at most once per
/// [`FkReverseCache`](crate::table::fk_reverse_cache::FkReverseCache) invalidation, via
/// [`FkReverseCache::get_or_build_by_parent`](crate::table::fk_reverse_cache::FkReverseCache::get_or_build_by_parent).
///
/// The walk only READS FK declarations, so it resolves through the unfenced
/// accessor ([`TableResolver::resolve_unfenced`](crate::table::TableResolver::resolve_unfenced)) and stays correct while a
/// rename is in doubt (a frozen source's declarations and data are stable).
pub async fn build_reverse_fk_entries(
    resolver: &dyn BatchResolver,
    repo_name: &str,
) -> shamir_storage::error::DbResult<Vec<TaggedReverseFkEntry>> {
    let repo = resolver.resolve_repo(repo_name).await?;
    let table_names = repo.list_table_names();
    let mut entries = Vec::new();

    for name in &table_names {
        let child_ref = TableRef::with_repo(repo_name, name);
        // F-55 (#881): a resolve failure on any single child table aborts
        // the WHOLE repo-wide scan instead of silently treating that table
        // as "declares no foreign keys". Swallowing the error here would
        // publish an incomplete FK graph via the caller's CAS, allowing a
        // subsequent parent UPDATE/DELETE to violate referential integrity
        // by never discovering the missed child as a dependent.
        // The walk only READS FK declarations, so it resolves through the
        // unfenced accessor and stays correct while a rename is in doubt (a
        // frozen source's declarations and data are stable).
        let child_table = resolver.resolve_unfenced(&child_ref).await?;

        for (field_path, fk) in child_table.collect_fk_refs() {
            entries.push(TaggedReverseFkEntry {
                parent_table: fk.ref_table.clone(),
                entry: ReverseFkEntry {
                    child_table: name.clone(),
                    child_field: field_path.join("."),
                    parent_ref_field: fk.ref_field.clone(),
                    on_delete: fk.on_delete,
                    on_update: fk.on_update,
                },
            });
        }
    }

    Ok(entries)
}
