//! Pre-computed permission cache for O(1) superadmin / O(n) access checks.

use crate::query::auth::{Action, Effect, Resource, Role};
use crate::query::batch::{BatchOp, QueryEntry};
use crate::query::filter::Filter;
use shamir_types::types::common::TMap;

// ============================================================================
// Core types
// ============================================================================

/// Pre-resolved permission decision.
#[derive(Debug, Clone)]
struct ResolvedPermission {
    action: Action,
    resource: Resource,
    effect: Effect,
}

/// Pre-computed permission cache built from a user's roles.
///
/// Resolves all role/permission conflicts once at construction time,
/// then provides fast access checks for individual operations or
/// entire batches.
///
/// **NOTE (architectural status):** This role-matrix RBAC + `row_filter`
/// RLS path is **test-only scaffolding**. The live access model is the
/// **Shomer DAC** (owner/group/mode, POSIX-style), enforced via
/// `ShamirDb::execute_as` -> `authorize_access` -> `permits`.
/// In the Shomer model, **groups replace roles** for coarse-grained
/// access; row-level security is a future Shomer feature
/// (`ResourcePath::Record`-level meta). This type and its companion
/// `execute_batch_with_permissions` are retained for engine-level unit
/// tests only and are NOT wired into the server's live request path.
#[derive(Debug, Clone)]
pub struct SessionPermissions {
    /// Fast path: if any role grants Action::All on Resource::Global, this is true.
    is_superadmin: bool,
    /// Pre-resolved permission decisions, sorted by specificity (desc).
    decisions: Vec<ResolvedPermission>,
    /// Pre-merged row filters per (action, resource).
    row_filters: Vec<(Action, Resource, Filter)>,
}

// ============================================================================
// Build
// ============================================================================

/// All individual actions (excluding All, which is expanded).
const EXPANDED_ACTIONS: [Action; 9] = [
    Action::Read,
    Action::Insert,
    Action::Update,
    Action::Delete,
    Action::Create,
    Action::Drop,
    Action::Alter,
    Action::ManageUsers,
    Action::ManageRoles,
];

impl SessionPermissions {
    /// Build from a slice of roles. Resolves all conflicts once.
    ///
    /// 1. Collects all permissions from all roles.
    /// 2. Expands `Action::All` into individual actions.
    /// 3. Stores as flat `Vec<ResolvedPermission>` sorted by specificity (desc).
    /// 4. Merges row_filters: same resource+action -> OR.
    /// 5. Detects superadmin (Action::All + Resource::Global + Allow).
    pub fn build(roles: &[Role]) -> Self {
        let mut is_superadmin = false;
        let mut decisions = Vec::new();
        let mut raw_filters: Vec<(Action, Resource, Option<Filter>)> = Vec::new();

        for role in roles {
            for perm in &role.permissions {
                // Check superadmin: Allow + All + Global
                if perm.effect == Effect::Allow
                    && perm.actions.contains(&Action::All)
                    && perm.resource == Resource::Global
                {
                    is_superadmin = true;
                }

                // Expand actions
                let actions = expand_actions(&perm.actions);

                for action in &actions {
                    decisions.push(ResolvedPermission {
                        action: *action,
                        resource: perm.resource.clone(),
                        effect: perm.effect,
                    });

                    // Track row filters for Allow permissions
                    if perm.effect == Effect::Allow {
                        raw_filters.push((*action, perm.resource.clone(), perm.row_filter.clone()));
                    }
                }
            }
        }

        // Sort by specificity descending for early exit in check()
        decisions.sort_by_key(|b| std::cmp::Reverse(b.resource.specificity()));

        // Merge row filters: group by (action, resource), OR them together.
        let row_filters = merge_row_filters(raw_filters);

        SessionPermissions {
            is_superadmin,
            decisions,
            row_filters,
        }
    }

    // ========================================================================
    // Check
    // ========================================================================

    /// Check if an action is allowed on a resource.
    ///
    /// Returns `Effect::Allow` for superadmin instantly. For others, scans
    /// the pre-resolved decisions and picks the most specific match. At
    /// equal specificity, Deny wins.
    pub fn check(&self, action: Action, resource: &Resource) -> Effect {
        if self.is_superadmin {
            return Effect::Allow;
        }

        let mut best: Option<(u8, Effect)> = None;

        for resolved in &self.decisions {
            if resolved.action.matches(action) && resolved.resource.covers(resource) {
                let spec = resolved.resource.specificity();
                match &best {
                    None => best = Some((spec, resolved.effect)),
                    Some((best_spec, _best_effect)) => {
                        if spec > *best_spec
                            || (spec == *best_spec && resolved.effect == Effect::Deny)
                        {
                            best = Some((spec, resolved.effect));
                        }
                    }
                }
            }
        }

        best.map(|(_, effect)| effect).unwrap_or(Effect::Deny)
    }

    /// Get the merged row filter for a specific action+resource.
    ///
    /// Finds all matching Allow permissions that have row_filters and
    /// ORs them together. If any matching Allow permission has no
    /// row_filter, returns `None` (no restriction).
    pub fn row_filter(&self, action: Action, resource: &Resource) -> Option<Filter> {
        if self.is_superadmin {
            return None;
        }

        // Check if any Allow permission without a filter covers this action+resource
        // (means unrestricted access at that level).
        for (rf_action, rf_resource, _) in &self.row_filters {
            if rf_action.matches(action) && rf_resource.covers(resource) {
                // This entry came from a permission with no filter — unrestricted
                // (merge_row_filters only keeps entries that collapsed to no filter
                //  or have a concrete filter). We need a different approach.
            }
        }

        // Collect all matching filters from the raw merged list
        let mut filters = Vec::new();
        let mut has_unrestricted = false;

        for (rf_action, rf_resource, filter) in &self.row_filters {
            if *rf_action == action && rf_resource.covers(resource) {
                match filter {
                    Filter::And { filters: fs } if fs.is_empty() => {
                        // Sentinel for "no restriction"
                        has_unrestricted = true;
                    }
                    _ => {
                        filters.push(filter.clone());
                    }
                }
            }
        }

        if has_unrestricted {
            return None;
        }

        match filters.len() {
            0 => None,
            1 => Some(filters.into_iter().next().unwrap()),
            _ => Some(Filter::Or { filters }),
        }
    }

    // ========================================================================
    // Batch check
    // ========================================================================

    /// The merged row-level-security filter that must be AND-ed into this op's
    /// WHERE clause for the current session, or `None` when unrestricted
    /// (superadmin, no matching row_filter grant, or an unrestricted Allow).
    /// Only data ops (Read/Update/Delete/Insert/Set) can be restricted; DDL ops
    /// naturally yield `None`.
    pub fn row_filter_for_op(&self, op: &BatchOp, db_name: &str) -> Option<Filter> {
        let (action, resource) = Self::extract_action_resource(op, db_name);
        self.row_filter(action, &resource)
    }

    /// Check all operations in a batch. Returns first denied operation or Ok(()).
    ///
    /// The `db_name` parameter provides database context for building
    /// `Resource::Table` from `TableRef` (which lacks a database field).
    pub fn check_batch(
        &self,
        queries: &TMap<String, QueryEntry>,
        db_name: &str,
    ) -> Result<(), (String, Action, Resource)> {
        for (alias, entry) in queries {
            let (action, resource) = Self::extract_action_resource(&entry.op, db_name);
            if self.check(action, &resource) == Effect::Deny {
                return Err((alias.clone(), action, resource));
            }
        }
        Ok(())
    }

    /// Extract action and resource from a BatchOp.
    fn extract_action_resource(op: &BatchOp, db_name: &str) -> (Action, Resource) {
        match op {
            // Data operations — use table_ref
            BatchOp::Read(_) => {
                let resource = table_ref_to_resource(op.table_ref().unwrap(), db_name);
                (Action::Read, resource)
            }
            BatchOp::Insert(_) => {
                let resource = table_ref_to_resource(op.table_ref().unwrap(), db_name);
                (Action::Insert, resource)
            }
            BatchOp::Update(_) => {
                let resource = table_ref_to_resource(op.table_ref().unwrap(), db_name);
                (Action::Update, resource)
            }
            BatchOp::Delete(_) => {
                let resource = table_ref_to_resource(op.table_ref().unwrap(), db_name);
                (Action::Delete, resource)
            }
            BatchOp::Set(_) => {
                let resource = table_ref_to_resource(op.table_ref().unwrap(), db_name);
                (Action::Update, resource)
            }

            // DDL — Create/Drop
            BatchOp::CreateDb(op) => (
                Action::Create,
                Resource::Database {
                    database: op.create_db.clone(),
                },
            ),
            BatchOp::DropDb(op) => (
                Action::Drop,
                Resource::Database {
                    database: op.drop_db.clone(),
                },
            ),
            BatchOp::CreateRepo(op) => (
                Action::Create,
                Resource::Repo {
                    database: db_name.to_string(),
                    repo: op.create_repo.clone(),
                },
            ),
            BatchOp::DropRepo(op) => (
                Action::Drop,
                Resource::Repo {
                    database: db_name.to_string(),
                    repo: op.drop_repo.clone(),
                },
            ),
            BatchOp::CreateTable(op) => (
                Action::Create,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.create_table.clone(),
                },
            ),
            BatchOp::DropTable(op) => (
                Action::Drop,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.drop_table.clone(),
                },
            ),
            BatchOp::CreateIndex(op) => (
                Action::Create,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.table.clone(),
                },
            ),
            BatchOp::DropIndex(op) => (
                Action::Drop,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.table.clone(),
                },
            ),

            // Per-table buffer config — read for get, alter/update
            // for set + alter. The table-level Alter action is
            // already used by index DDL; reusing keeps the role
            // matrix small and predictable.
            BatchOp::GetBufferConfig(op) => (
                Action::Read,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.get_buffer_config.clone(),
                },
            ),
            BatchOp::SetBufferConfig(op) => (
                Action::Alter,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.set_buffer_config.clone(),
                },
            ),
            BatchOp::AlterBufferConfig(op) => (
                Action::Alter,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.alter_buffer_config.clone(),
                },
            ),

            // Auth management
            BatchOp::CreateUser(_)
            | BatchOp::DropUser(_)
            | BatchOp::GrantRole(_)
            | BatchOp::RevokeRole(_) => (Action::ManageUsers, Resource::Global),
            BatchOp::CreateRole(_) | BatchOp::DropRole(_) => {
                (Action::ManageRoles, Resource::Global)
            }

            // Migration — alter on the source table (destructive DDL)
            BatchOp::StartMigration(op) => (
                Action::Alter,
                Resource::Table {
                    database: db_name.to_string(),
                    repo: op.repo.clone(),
                    table: op.start_migration.clone(),
                },
            ),
            BatchOp::CommitMigration(_)
            | BatchOp::RollbackMigration(_)
            | BatchOp::MigrationStatus(_) => (Action::Alter, Resource::Global),

            // List + access-tree introspection — read on global
            BatchOp::List(_) | BatchOp::AccessTree(_) => (Action::Read, Resource::Global),

            // Access-control DDL (S3) — admin manage operations
            BatchOp::Chmod(_)
            | BatchOp::Chown(_)
            | BatchOp::Chgrp(_)
            | BatchOp::CreateGroup(_)
            | BatchOp::DropGroup(_)
            | BatchOp::AddGroupMember(_)
            | BatchOp::RemoveGroupMember(_) => (Action::Alter, Resource::Global),

            // Function / validator / folder DDL (DDL-A) — admin alter ops
            BatchOp::CreateFunction(_)
            | BatchOp::DropFunction(_)
            | BatchOp::RenameFunction(_)
            | BatchOp::CreateValidator(_)
            | BatchOp::DropValidator(_)
            | BatchOp::RenameValidator(_)
            | BatchOp::BindValidator(_)
            | BatchOp::UnbindValidator(_)
            | BatchOp::ListValidators(_)
            | BatchOp::CreateFunctionFolder(_) => (Action::Alter, Resource::Global),

            // Stored procedure call — execute on a function, no table_ref.
            BatchOp::Call(_) => (Action::Read, Resource::Global),
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Expand `Action::All` into all individual actions.
fn expand_actions(actions: &[Action]) -> Vec<Action> {
    let mut result = Vec::new();
    for action in actions {
        if *action == Action::All {
            result.extend_from_slice(&EXPANDED_ACTIONS);
        } else {
            result.push(*action);
        }
    }
    result
}

/// Convert a `TableRef` + db_name into a `Resource::Table`.
fn table_ref_to_resource(table_ref: &crate::query::TableRef, db_name: &str) -> Resource {
    Resource::Table {
        database: db_name.to_string(),
        repo: table_ref.repo.clone(),
        table: table_ref.table.clone(),
    }
}

/// Merge row filters: group by (action, resource), OR them together.
///
/// If any entry in the group has `None` filter (unrestricted), the entire
/// group is unrestricted. We represent this as an empty `And` sentinel
/// so that `row_filter()` can detect it.
fn merge_row_filters(
    raw: Vec<(Action, Resource, Option<Filter>)>,
) -> Vec<(Action, Resource, Filter)> {
    // Group by (action, resource)
    let mut groups: Vec<(Action, Resource, Vec<Option<Filter>>)> = Vec::new();

    for (action, resource, filter) in raw {
        let found = groups
            .iter_mut()
            .find(|(a, r, _)| *a == action && *r == resource);
        match found {
            Some((_, _, filters)) => filters.push(filter),
            None => groups.push((action, resource, vec![filter])),
        }
    }

    let mut result = Vec::new();
    for (action, resource, filters) in groups {
        // If any filter is None → unrestricted
        if filters.iter().any(|f| f.is_none()) {
            // Sentinel: empty And means unrestricted
            result.push((action, resource, Filter::And { filters: vec![] }));
            continue;
        }

        // All filters are Some — OR them together
        let concrete: Vec<Filter> = filters.into_iter().map(|f| f.unwrap()).collect();
        let merged = match concrete.len() {
            1 => concrete.into_iter().next().unwrap(),
            _ => Filter::Or { filters: concrete },
        };
        result.push((action, resource, merged));
    }

    result
}
