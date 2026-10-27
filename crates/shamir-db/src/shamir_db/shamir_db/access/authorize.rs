use shamir_collections::TFxMap;
use shamir_engine::function::{FunctionMeta, Security};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::access::{
    permits, trace_access, AccessError, Action, Actor, Mode, ResourceMeta, ResourcePath,
    OWNER_SYSTEM,
};
use crate::DbResult;

use super::super::ShamirDb;
use super::gate::access_node;

impl ShamirDb {
    /// Enforcing authorization gate.
    ///
    /// Performs the full POSIX-style check:
    /// 1. `Actor::System` → `Ok` immediately (admin bypass, zero overhead
    ///    beyond the branch — the common live path).
    /// 2. **Traversal**: for each ancestor in `path.ancestors()` (nearest →
    ///    Root), the actor needs `Execute` on it. Resolves meta, computes
    ///    `in_group`, and checks [`permits`].
    /// 3. **Target**: resolves `resource_meta(path)`, computes `in_group`,
    ///    and checks [`permits`] for the requested `action`.
    ///
    /// On denial, builds an [`AccessError`] identifying the actor, the
    /// denied path, and the action. The engine-level observability trace
    /// ([`trace_access`]) is still emitted first — it is NOT part of the
    /// enforcement decision (it always returns `Ok`); the real check is
    /// everything below this call.
    pub async fn authorize_access(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError> {
        self.authorize_access_in(actor, path, action, None).await
    }

    pub(super) async fn authorize_access_in(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
        memo: Option<&super::batch_gate::AclMemo>,
    ) -> Result<(), AccessError> {
        // Engine-level observability trace (R2) — always emitted, always Ok.
        // NOT the enforcement gate; see `trace_access`'s doc comment.
        trace_access(actor, path, action)?;

        // Admin bypass — the common live path. Both `System` (anonymous
        // default) and `Admin(_)` (a real superuser session carrying its
        // principal64 id) short-circuit the gate; `Admin` differs from
        // `System` only in ownership attribution, not in gate semantics.
        if matches!(actor, Actor::System | Actor::Admin(_)) {
            return Ok(());
        }

        let user_id = match actor {
            Actor::User(id) => *id,
            Actor::System | Actor::Admin(_) => unreachable!(),
        };

        // Traversal: each ancestor needs Execute.
        for anc in path.ancestors() {
            let anc_meta = match Self::meta_memo(self, &anc, memo).await {
                Ok(m) => m,
                Err(e) => {
                    // Fail-closed: a real catalogue-read error on an
                    // ancestor must deny unconditionally — never fall
                    // through to a default-open `permits` check.
                    log::warn!(
                        "authorize_access: resource_meta failed for ancestor '{}' \
                         (actor={actor}, action={}): {e}",
                        anc,
                        Action::Execute
                    );
                    return Err(AccessError {
                        actor: actor.clone(),
                        path: anc.to_string(),
                        action: Action::Execute,
                    });
                }
            };
            let in_group = self.resolve_in_group(user_id, &anc_meta, memo).await;
            if !permits(actor, &anc_meta, Action::Execute, in_group) {
                return Err(AccessError {
                    actor: actor.clone(),
                    path: anc.to_string(),
                    action: Action::Execute,
                });
            }
        }

        // Target check.
        let meta = match Self::meta_memo(self, path, memo).await {
            Ok(m) => m,
            Err(e) => {
                // Fail-closed: a real catalogue-read error on the target
                // must deny unconditionally — never fall through to a
                // default-open `permits` check.
                log::warn!(
                    "authorize_access: resource_meta failed for target '{}' \
                     (actor={actor}, action={action}): {e}",
                    path
                );
                return Err(AccessError {
                    actor: actor.clone(),
                    path: path.to_string(),
                    action,
                });
            }
        };
        let in_group = self.resolve_in_group(user_id, &meta, memo).await;
        if permits(actor, &meta, action, in_group) {
            Ok(())
        } else {
            Err(AccessError {
                actor: actor.clone(),
                path: path.to_string(),
                action,
            })
        }
    }

    async fn meta_memo(
        &self,
        path: &ResourcePath,
        memo: Option<&super::batch_gate::AclMemo>,
    ) -> DbResult<ResourceMeta> {
        if let Some(memo) = memo {
            if let Some(meta) = memo.metas.get(path).map(|entry| entry.clone()) {
                return Ok(meta);
            }
        }
        let meta = self.resource_meta(path).await?;
        if let Some(memo) = memo {
            memo.metas.insert(path.clone(), meta.clone());
        }
        Ok(meta)
    }

    /// Resolve whether the user belongs to the group specified in `meta`.
    ///
    /// Returns `false` if the meta has no group or the lookup fails.
    async fn resolve_in_group(
        &self,
        user_id: u64,
        meta: &ResourceMeta,
        memo: Option<&super::batch_gate::AclMemo>,
    ) -> bool {
        match meta.group {
            Some(gid) => {
                if let Some(memo) = memo {
                    if let Some(value) = memo.membership.get(&(user_id, gid)).map(|entry| *entry) {
                        return value;
                    }
                }
                match self.user_in_group(user_id, gid).await {
                    Ok(value) => {
                        if let Some(memo) = memo {
                            memo.membership.insert((user_id, gid), value);
                        }
                        value
                    }
                    Err(_) => false,
                }
            }
            None => false,
        }
    }

    /// Resolve the effective actor for function invocation.
    ///
    /// Combines two previously-independent mechanisms into one decision:
    ///
    /// - **`FunctionMeta::security`** (CRIT-6 part B / audit #440): the
    ///   declared `SECURITY INVOKER` / `SECURITY DEFINER` semantics, stored
    ///   in the catalogue. This is the *modern, explicit* declaration.
    /// - **The POSIX setuid mode bit** on `ResourceMeta`: a legacy,
    ///   bit-level escalation flag carried over from the pre-slice-9
    ///   access model.
    ///
    /// # Decision table
    ///
    /// | `security`   | setuid bit | effective actor     |
    /// |--------------|------------|---------------------|
    /// | `Definer`    | (any)      | function **owner**  |
    /// | `Invoker`    | set        | function **owner** (legacy) |
    /// | `Invoker`    | clear      | **caller**          |
    ///
    /// ## Why `Definer` always escalates
    ///
    /// `SECURITY DEFINER` is an explicit, unconditional request for
    /// owner-privilege execution (mirrors Postgres `SECURITY DEFINER`):
    /// the function runs as its owner regardless of the legacy mode bit.
    /// Before CRIT-6 this declaration was dead weight — it now drives
    /// the decision.
    ///
    /// ## Why `Invoker` still honours the setuid bit
    ///
    /// Two design alternatives were considered:
    ///
    /// 1. **Strict** — `Invoker` is a hard, unblockable guarantee that the
    ///    function runs as the caller, ignoring the setuid bit entirely.
    ///    Most secure in isolation, but it silently changes the runtime
    ///    behaviour of every pre-existing function whose catalogue row
    ///    lacks an explicit `security` field (those default to `Invoker`)
    ///    yet relied on the setuid bit to escalate — a quiet, broad
    ///    behavioural regression.
    /// 2. **Legacy-compatible** (chosen) — an explicit `Invoker`
    ///    declaration still honours the setuid bit, preserving the
    ///    pre-CRIT-6 behaviour for callers that depended on the mode bit
    ///    alone. The primary defect (that `Security::Definer` was never
    ///    applied) is closed because `Definer` now unconditionally
    ///    escalates. The setuid bit becomes a legacy escalation path that
    ///    is *redundant* under `Definer` and *preserved* under `Invoker`
    ///    for backward compatibility.
    ///
    /// This keeps the existing setuid-driven test
    /// (`effective_fn_actor_switches_on_setuid`, which models a function
    /// with no explicit `security` and the setuid bit set) green while
    /// still making `Security::Definer` a real, enforced declaration.
    ///
    /// # Fail-closed guarantee
    ///
    /// Privilege escalation only happens when the function record is
    /// **definitively loaded** from the catalogue AND carries an explicit,
    /// present `owner` field. The caller is returned unchanged — never
    /// `Actor::System` via a `ResourceMeta`/`from_record` default — in
    /// EITHER of two cases:
    ///
    /// - the record itself is absent, or the load errors (`load_function`
    ///   returns anything other than `Ok(Some(_))`); or
    /// - the record loads but its `owner` field is missing (a legacy
    ///   record predating the field, or a partially-written/corrupted
    ///   one) — `ResourceMeta::from_record`'s `unwrap_or(Actor::System)`
    ///   default is deliberately correct for every OTHER caller (DDL
    ///   introspection, `access_tree`, `resource_meta`) but would silently
    ///   escalate here, so escalation reads the owner via
    ///   [`ResourceMeta::owner_field`] instead, which distinguishes
    ///   "absent" (`None`) from "explicitly System" (`Some(Actor::System)`).
    pub async fn effective_fn_actor(&self, fn_name: &str, caller: &Actor) -> Actor {
        // Load the raw function record directly so we can distinguish
        // "record found" from "error / not present" (the latter must not
        // escalate the caller to the open()-default owner of System).
        let Ok(Some(rec)) = self.system_store.load_function(fn_name).await else {
            return caller.clone();
        };
        let res_meta = ResourceMeta::from_record(&rec);
        let fn_meta = FunctionMeta::from_record(&rec);
        // Fail-closed owner lookup for escalation: `None` (owner field
        // absent) must resolve to the caller, never to
        // `from_record`'s System default. See the doc comment above.
        let escalated_owner = || ResourceMeta::owner_field(&rec).unwrap_or_else(|| caller.clone());
        match fn_meta.security {
            // Explicit definer request → always run as the function owner,
            // irrespective of the legacy POSIX setuid mode bit.
            Security::Definer => escalated_owner(),
            // Explicit (or defaulted) invoker: honour the legacy setuid
            // bit for backward compatibility — see the doc note above.
            Security::Invoker => {
                if Mode::is_setuid(res_meta.mode) {
                    escalated_owner()
                } else {
                    caller.clone()
                }
            }
        }
    }

    /// Assemble the access-control tree as a structured [`QueryValue`] map.
    ///
    /// Shape (see [`shamir_query_types::admin::AccessTreeOp`]):
    /// ```text
    /// {
    ///   "resources": { "name": "/", "kind": "root", "owner": 0,
    ///                  "owner_name": "system", "group": null,
    ///                  "group_name": null, "mode": 511, "setuid": false,
    ///                  "children": [ /* databases → stores → tables */ ] },
    ///   "functions": [ { "name": "...", "owner": .., "mode": .., "setuid": .. } ],
    ///   "principals": {
    ///     "users":  [ { "id": .., "name": ".." } ],
    ///     "groups": [ { "id": .., "name": "..", "members": [ {id,name} ] } ]
    ///   }
    /// }
    /// ```
    ///
    /// `depth` caps the resource hierarchy (`0`=root, `1`=databases,
    /// `2`=stores, `3`=tables; `None`=full). `db_filter` restricts the
    /// resource tree to one database. Pure read-only assembly — the admin
    /// gate is applied by the caller (the DDL dispatch authorizes `Manage`
    /// on the root; the offline CLI runs as `System`).
    pub async fn access_tree(
        &self,
        depth: Option<u32>,
        db_filter: Option<&str>,
    ) -> DbResult<QueryValue> {
        // ── principals first, so resource nodes resolve owner/group names ──
        let mut name_of: TFxMap<u64, String> = TFxMap::default();
        name_of.insert(OWNER_SYSTEM, "system".to_string());
        let mut users_list: Vec<QueryValue> = Vec::new();
        // Task #559: enumerate principals from the injected
        // PrincipalResolver (the real directory) instead of Store B's
        // `users_table()`. With no resolver installed the principals
        // section is empty — the design doc's "absent resolver → names
        // resolve to null" degrade (NOT the old hash-based bridge).
        if let Some(resolver) = self.principal_resolver() {
            for p in resolver.list() {
                name_of.insert(p.principal64, p.name.clone());
                let mut m = new_map();
                m.insert("id".to_string(), QueryValue::Int(p.principal64 as i64));
                m.insert("name".to_string(), QueryValue::Str(p.name));
                users_list.push(QueryValue::Map(m));
            }
        }
        users_list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        let mut group_name_of: TFxMap<u64, String> = TFxMap::default();
        let mut groups_list: Vec<QueryValue> = Vec::new();
        for rec in self.system_store.load_groups().await? {
            let Some(gid) = rec.get("group_id").and_then(|v| v.as_u64()) else {
                continue;
            };
            let gname = rec
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            group_name_of.insert(gid, gname.clone());
            let members: Vec<QueryValue> = rec
                .get("members")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.as_u64())
                        .map(|m| {
                            let mut mm = new_map();
                            mm.insert("id".to_string(), QueryValue::Int(m as i64));
                            mm.insert(
                                "name".to_string(),
                                name_of
                                    .get(&m)
                                    .map(|n| QueryValue::Str(n.clone()))
                                    .unwrap_or(QueryValue::Null),
                            );
                            QueryValue::Map(mm)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut gm = new_map();
            gm.insert("id".to_string(), QueryValue::Int(gid as i64));
            gm.insert("name".to_string(), QueryValue::Str(gname));
            gm.insert("members".to_string(), QueryValue::List(members));
            groups_list.push(QueryValue::Map(gm));
        }
        groups_list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        // ── resource hierarchy (Root → Database → Store → Table) ──
        let max_depth = depth.unwrap_or(3).min(3);
        let root_meta = self.resource_meta(&ResourcePath::Root).await?;
        let mut root = access_node("/", "root", &root_meta, &name_of, &group_name_of);

        if max_depth >= 1 {
            let dbs: Vec<String> = match db_filter {
                Some(d) => self.list_dbs().into_iter().filter(|x| x == d).collect(),
                None => self.list_dbs(),
            };
            let mut db_children: Vec<QueryValue> = Vec::new();
            for dbname in dbs {
                let dm = self.resource_meta(&ResourcePath::database(&dbname)).await?;
                let mut dbnode = access_node(&dbname, "database", &dm, &name_of, &group_name_of);
                if max_depth >= 2 {
                    if let Some(inst) = self.get_db(&dbname) {
                        let mut store_children: Vec<QueryValue> = Vec::new();
                        for store in inst.list_repos() {
                            let sm = self
                                .resource_meta(&ResourcePath::store(&dbname, &store))
                                .await?;
                            let mut snode =
                                access_node(&store, "store", &sm, &name_of, &group_name_of);
                            if max_depth >= 3 {
                                if let Ok(tables) = inst.list_tables(&store) {
                                    let mut tnodes: Vec<QueryValue> = Vec::new();
                                    for t in tables {
                                        let tm = self
                                            .resource_meta(&ResourcePath::table(
                                                &dbname, &store, &t,
                                            ))
                                            .await?;
                                        tnodes.push(access_node(
                                            &t,
                                            "table",
                                            &tm,
                                            &name_of,
                                            &group_name_of,
                                        ));
                                    }
                                    if let QueryValue::Map(ref mut m) = snode {
                                        m.insert("children".to_string(), QueryValue::List(tnodes));
                                    }
                                }
                            }
                            store_children.push(snode);
                        }
                        if let QueryValue::Map(ref mut m) = dbnode {
                            m.insert("children".to_string(), QueryValue::List(store_children));
                        }
                    }
                }
                db_children.push(dbnode);
            }
            if let QueryValue::Map(ref mut m) = root {
                m.insert("children".to_string(), QueryValue::List(db_children));
            }
        }

        // ── functions (flat for now; folders land in a later slice) ──
        let mut functions: Vec<QueryValue> = Vec::new();
        for fname in self.list_functions().await? {
            let fm = self.resource_meta(&ResourcePath::function(&fname)).await?;
            let mut fnode = access_node(&fname, "function", &fm, &name_of, &group_name_of);
            if let QueryValue::Map(ref mut m) = fnode {
                m.swap_remove("children");
                m.insert(
                    "builtin".to_string(),
                    QueryValue::Bool(self.function_meta(&fname).is_none()),
                );
            }
            functions.push(fnode);
        }

        let mut principals = new_map();
        principals.insert("users".to_string(), QueryValue::List(users_list));
        principals.insert("groups".to_string(), QueryValue::List(groups_list));

        let mut result = new_map();
        result.insert("resources".to_string(), root);
        result.insert("functions".to_string(), QueryValue::List(functions));
        result.insert("principals".to_string(), QueryValue::Map(principals));

        Ok(QueryValue::Map(result))
    }
}
