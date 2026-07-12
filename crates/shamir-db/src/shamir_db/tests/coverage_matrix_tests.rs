//! #546 gate-coverage recommendation 2 — object×op coverage matrix.
//!
//! `docs/roadmap/ACCESS_HIERARCHY.md` documents the full securable-object ×
//! operation matrix the Shomer DAC model is supposed to enforce. This test
//! drives that matrix as DATA (a table of `(object kind, Action)` pairs
//! transcribed 1:1 from the doc's own table) against the REAL, single
//! enforcement funnel every wire-reachable path bottoms out in:
//! `ShamirDb::authorize_access`.
//!
//! Why `authorize_access` and not a hand-built `BatchOp` per cell: every
//! dispatcher handler (`execute_as`/`tx_execute_as`'s per-op loop via
//! `BatchOp::required_access`, and every `admin_*.rs` DDL handler) calls
//! `authorize_access(&actor, &path, action)` directly with the SAME
//! `(ResourcePath, Action)` pair this matrix lists — see the grep audit in
//! this task's closing report. Driving `authorize_access` itself therefore
//! exercises the actual enforcement decision every real caller depends on,
//! without needing a hand-built `BatchOp`/wire round-trip per matrix cell
//! (which `facade_gateway_acl_tests.rs` already covers end-to-end for the
//! Table-Read/Write cells specifically, proving the wire path reaches this
//! same gate). The WASM `db_execute` gateway (`FacadeDbGateway`) has no
//! independent gate of its own — every one of its methods calls
//! `execute_as(self.actor.clone(), ...)`, so it inherits this same coverage
//! (confirmed by grep: `db_gateway.rs`'s 4 `execute_as` call sites).
//!
//! Matrix source (`docs/roadmap/ACCESS_HIERARCHY.md`'s "Object × operation
//! matrix" table), transcribed as `(object, Action)` cells that denote a
//! REAL restriction (a `—` cell in the doc means "not applicable to this
//! object" and is skipped):
//!
//! | Object            | Read | Write | Create | Delete | Execute | List | Manage |
//! |--------------------|------|-------|--------|--------|---------|------|--------|
//! | Root               |  —   |  —    |  (open)|   —    |   —     |(open)|   X    |
//! | Database            |  X   |  —    |  X     |   X    |   X     |  X   |   X    |
//! | Store                |  X   |  —    |  X     |   X    |   X     |  X   |   X    |
//! | Table                 |  X   |  X    |  (X)   |   X    |   X     |  —   |   X    |
//! | Record                 |  X   |  X    |  —     |   X    |   —     |  —   | (inherit)|
//! | Index                   |  —   |  X    |  —     |   X    |   —     |  —   | (inherit)|
//! | FunctionNamespace         |(open)|  —    |(open) |   —    |   —     |(open)|   —    |
//! | Function                   |(open)|(open) |  —    |(open)  |(open)   |  —   |   X    |
//! | User                        |(open)|(open) |  —    |(open)  |   —     |  —   |   X    |
//! | Group                        |(open)|  —    |  —    |(open)  |   —     |(open)|   X    |
//!
//! `X` cells assert DENIAL for a no-rights actor (real mode-bit enforcement
//! against an owner-rwx-only default). `(open)` cells are NOT asserted as
//! denied — `resource_meta` resolves these paths to `ResourceMeta::open()`
//! unconditionally (`Root`/`User`/`Group`: hard-coded `Ok(open())` arm in
//! `resource_meta`; `FunctionNamespace`/`Function`-with-no-catalogue-row:
//! `Ok(None) => ResourceMeta::default()` on a genuinely-absent record), so
//! mode-bit-gated actions on them are allowed for EVERY actor today. This
//! is a pre-existing, deliberate (if debatable) posture — see
//! `ResourceMeta::open`'s doc comment ("so nothing is restricted while the
//! gate is transparent") and task #550 ("Root/User/Group always-open meta"),
//! which owns the decision of whether to close it. This test asserts the
//! matrix AS ENFORCED TODAY (proving the real `X` cells are genuinely
//! denied, including the ones item (a)'s `required_access` refactor and
//! item (d)'s group-CRUD gate touch) and explicitly documents the `(open)`
//! cells as known, tracked gaps rather than silently omitting them — a
//! future #550 fix that closes one of these should make the corresponding
//! `(open)` cell start failing THIS test's `_allows_open_default_cells`
//! companion below, forcing an explicit update here.
//!
//! Record/Index inherit their Table's meta (`resource_meta` resolves them to
//! the Table path) — covered by `enforcement_tests::record_enforcement_inherits_table_meta`
//! already; this matrix focuses on the mode-bearing objects that carry their
//! OWN meta (the inheritance property is orthogonal to "is this cell
//! enforced at all"). `Manage` is the one action `permits` never grants via
//! mode bits regardless of the `open()` default (owner-only, unconditional
//! — the default owner is `Actor::System`, never a `User(_)`), so it is the
//! one column asserted as denied even on every `(open)`-row object.

use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::ShamirDb;
use shamir_types::access::{Action, ResourcePath};

/// One matrix cell: a human-readable label, the resource path (built once
/// `testdb`/`main`/`items` exist), and the action that must be denied for a
/// no-rights actor.
struct MatrixCell {
    label: &'static str,
    path: fn() -> ResourcePath,
    action: Action,
}

/// The `X` cells of the object × operation matrix — real mode-bit
/// enforcement, must DENY a no-rights actor. Transcribed from
/// `ACCESS_HIERARCHY.md`; each entry names the doc's `Object`/column pair.
fn x_cells() -> Vec<MatrixCell> {
    vec![
        // ── Root — only Manage is enforced (owner-only); Create/List are
        // open-by-design, see `open_cells` below and the module doc. ──
        MatrixCell {
            label: "Root/Manage (server admin)",
            path: || ResourcePath::Root,
            action: Action::Manage,
        },
        // ── Database ──────────────────────────────────────────────────
        MatrixCell {
            label: "Database/Read (describe)",
            path: || ResourcePath::database("testdb"),
            action: Action::Read,
        },
        MatrixCell {
            label: "Database/Create (create store/table)",
            path: || ResourcePath::database("testdb"),
            action: Action::Create,
        },
        MatrixCell {
            label: "Database/Delete (drop db)",
            path: || ResourcePath::database("testdb"),
            action: Action::Delete,
        },
        MatrixCell {
            label: "Database/Execute (traverse)",
            path: || ResourcePath::database("testdb"),
            action: Action::Execute,
        },
        MatrixCell {
            label: "Database/List (list stores/tables)",
            path: || ResourcePath::database("testdb"),
            action: Action::List,
        },
        MatrixCell {
            label: "Database/Manage (chmod/chown/chgrp)",
            path: || ResourcePath::database("testdb"),
            action: Action::Manage,
        },
        // ── Store ─────────────────────────────────────────────────────
        MatrixCell {
            label: "Store/Read (describe)",
            path: || ResourcePath::store("testdb", "main"),
            action: Action::Read,
        },
        MatrixCell {
            label: "Store/Create (create table)",
            path: || ResourcePath::store("testdb", "main"),
            action: Action::Create,
        },
        MatrixCell {
            label: "Store/Delete (drop store)",
            path: || ResourcePath::store("testdb", "main"),
            action: Action::Delete,
        },
        MatrixCell {
            label: "Store/Execute (traverse)",
            path: || ResourcePath::store("testdb", "main"),
            action: Action::Execute,
        },
        MatrixCell {
            label: "Store/List (list tables)",
            path: || ResourcePath::store("testdb", "main"),
            action: Action::List,
        },
        MatrixCell {
            label: "Store/Manage (chmod/chown/chgrp)",
            path: || ResourcePath::store("testdb", "main"),
            action: Action::Manage,
        },
        // ── Table ─────────────────────────────────────────────────────
        MatrixCell {
            label: "Table/Read (query rows)",
            path: || ResourcePath::table("testdb", "main", "items"),
            action: Action::Read,
        },
        MatrixCell {
            label: "Table/Write (insert/update/delete rows)",
            path: || ResourcePath::table("testdb", "main", "items"),
            action: Action::Write,
        },
        MatrixCell {
            label: "Table/Create (insert row / required_access(Insert))",
            path: || ResourcePath::table("testdb", "main", "items"),
            action: Action::Create,
        },
        MatrixCell {
            label: "Table/Delete (drop table)",
            path: || ResourcePath::table("testdb", "main", "items"),
            action: Action::Delete,
        },
        MatrixCell {
            label: "Table/Execute (traverse)",
            path: || ResourcePath::table("testdb", "main", "items"),
            action: Action::Execute,
        },
        MatrixCell {
            label: "Table/Manage (chmod/chown/chgrp)",
            path: || ResourcePath::table("testdb", "main", "items"),
            action: Action::Manage,
        },
        // ── Record (own-meta cells; inheritance covered separately) ──
        MatrixCell {
            label: "Record/Read (get row)",
            path: || ResourcePath::record("testdb", "main", "items", "key1"),
            action: Action::Read,
        },
        MatrixCell {
            label: "Record/Write (update row)",
            path: || ResourcePath::record("testdb", "main", "items", "key1"),
            action: Action::Write,
        },
        MatrixCell {
            label: "Record/Delete (delete row)",
            path: || ResourcePath::record("testdb", "main", "items", "key1"),
            action: Action::Delete,
        },
        // ── Index ─────────────────────────────────────────────────────
        MatrixCell {
            label: "Index/Write (rebuild)",
            path: || ResourcePath::index("testdb", "main", "items", "idx1"),
            action: Action::Write,
        },
        MatrixCell {
            label: "Index/Delete (drop index)",
            path: || ResourcePath::index("testdb", "main", "items", "idx1"),
            action: Action::Delete,
        },
        // ── FunctionNamespace — Create/List are open-by-design (see
        // `open_cells`); no Manage column in the doc for this object. ──
        // ── Function — only Manage is enforced (owner-only); a function
        // with no catalogue row resolves its meta via
        // `Ok(None) => ResourceMeta::default()` (genuinely-absent record),
        // so Read/Write/Delete/Execute are open-by-design — see
        // `open_cells` below. ──
        MatrixCell {
            label: "Function/Manage (chmod/chown/setuid)",
            path: || ResourcePath::function("myfn"),
            action: Action::Manage,
        },
        // ── User — only Manage is enforced (owner-only); User always
        // resolves to `ResourceMeta::open()` (hard-coded arm in
        // `resource_meta`) — see `open_cells` below and task #550. ──
        MatrixCell {
            label: "User/Manage (admin)",
            path: || ResourcePath::user("alice"),
            action: Action::Manage,
        },
        // ── Group — same open()-default caveat as User; Manage
        // (add/remove members) is the real cell — matches
        // `create_group_as`'s and friends' inline gate this task's item
        // (d) added. ──
        MatrixCell {
            label: "Group/Manage (add/remove members)",
            path: || ResourcePath::group("devs"),
            action: Action::Manage,
        },
    ]
}

/// The `(open)` cells of the matrix — resolve to `ResourceMeta::open()`
/// (or its `Ok(None)` -> `default()` equivalent for a genuinely-absent
/// catalogue row) UNCONDITIONALLY today, so a no-rights actor IS allowed
/// mode-bit-gated actions on them. Documented here (rather than silently
/// omitted) so a future #550 fix that closes one of these makes
/// `access_hierarchy_matrix_open_cells_are_currently_unenforced` below
/// start failing — forcing this file to be updated in lock-step with the
/// enforcement change, instead of the coverage matrix silently going
/// stale.
fn open_cells() -> Vec<MatrixCell> {
    vec![
        MatrixCell {
            label: "Root/Create (create db) — open by design",
            path: || ResourcePath::Root,
            action: Action::Create,
        },
        MatrixCell {
            label: "Root/List (list dbs) — open by design",
            path: || ResourcePath::Root,
            action: Action::List,
        },
        MatrixCell {
            label: "FunctionNamespace/Create (create function) — open by design",
            path: || ResourcePath::FunctionNamespace,
            action: Action::Create,
        },
        MatrixCell {
            label: "FunctionNamespace/List (list functions) — open by design",
            path: || ResourcePath::FunctionNamespace,
            action: Action::List,
        },
        MatrixCell {
            label: "Function/Read (describe/source) — open by design (no catalogue row)",
            path: || ResourcePath::function("myfn"),
            action: Action::Read,
        },
        MatrixCell {
            label: "Function/Write (alter) — open by design (no catalogue row)",
            path: || ResourcePath::function("myfn"),
            action: Action::Write,
        },
        MatrixCell {
            label: "Function/Delete (drop) — open by design (no catalogue row)",
            path: || ResourcePath::function("myfn"),
            action: Action::Delete,
        },
        MatrixCell {
            label: "Function/Execute (invoke) — open by design (no catalogue row)",
            path: || ResourcePath::function("myfn"),
            action: Action::Execute,
        },
        MatrixCell {
            label: "User/Read — open by design (ResourceMeta::open() always)",
            path: || ResourcePath::user("alice"),
            action: Action::Read,
        },
        MatrixCell {
            label: "User/Write — open by design (ResourceMeta::open() always)",
            path: || ResourcePath::user("alice"),
            action: Action::Write,
        },
        MatrixCell {
            label: "User/Delete — open by design (ResourceMeta::open() always)",
            path: || ResourcePath::user("alice"),
            action: Action::Delete,
        },
        MatrixCell {
            label: "Group/Read — open by design (ResourceMeta::open() always)",
            path: || ResourcePath::group("devs"),
            action: Action::Read,
        },
        MatrixCell {
            label: "Group/List — open by design (ResourceMeta::open() always)",
            path: || ResourcePath::group("devs"),
            action: Action::List,
        },
    ]
}

/// Set up a `ShamirDb` with `testdb/main/items` (+ an index) so every
/// matrix cell above resolves to a real, existing resource.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", config).await.unwrap();
    shamir
        .get_db("testdb")
        .unwrap()
        .create_index("main", "items", "idx1", &["field1"])
        .await
        .unwrap();
    shamir
}

/// The coverage matrix itself: for every REAL-enforcement `(object, Action)`
/// cell listed in `ACCESS_HIERARCHY.md`'s matrix (the `X` cells — see
/// `x_cells`), a no-rights `Actor::User` (no grants, no ownership, no group
/// membership — the enforced-by-default create posture, see
/// `ResourceMeta::owned_enforced`) must be DENIED by `authorize_access`,
/// the single real enforcement gate every wire-reachable path funnels
/// through. This is the test that would have caught a hypothetical future
/// regression in `BatchOp::required_access` (item (a)) or the group-CRUD
/// `Manage(Root)` inline gate (item (d)) failing to deny a stranger.
#[tokio::test]
async fn access_hierarchy_matrix_denies_no_rights_actor() {
    use shamir_types::access::Actor;

    let shamir = setup().await;
    let stranger = Actor::User(999_999);

    let cells = x_cells();
    assert!(
        cells.len() >= 20,
        "sanity: the matrix table should list every real object×op cell \
         from ACCESS_HIERARCHY.md (got {} — did a cell get dropped?)",
        cells.len()
    );

    let mut failures = Vec::new();
    for cell in &cells {
        let path = (cell.path)();
        let result = shamir.authorize_access(&stranger, &path, cell.action).await;
        if result.is_ok() {
            failures.push(format!(
                "{}: expected DENY for no-rights actor on {:?}/{:?}, got Ok",
                cell.label, path, cell.action
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "coverage-matrix gaps found ({} of {} cells wrongly allowed):\n{}",
        failures.len(),
        cells.len(),
        failures.join("\n")
    );
}

/// Companion: `Actor::System` bypasses EVERY `X` cell in the matrix — this
/// pins the admin-bypass invariant the whole gate relies on, so a future
/// regression that narrows the bypass doesn't silently break every admin
/// workflow while the no-rights-denial test above stays green.
#[tokio::test]
async fn access_hierarchy_matrix_allows_system_actor() {
    use shamir_types::access::Actor;

    let shamir = setup().await;

    let mut failures = Vec::new();
    for cell in x_cells().into_iter().chain(open_cells()) {
        let path = (cell.path)();
        let result = shamir
            .authorize_access(&Actor::System, &path, cell.action)
            .await;
        if result.is_err() {
            failures.push(format!(
                "{}: expected System to bypass on {:?}/{:?}, got {:?}",
                cell.label, path, cell.action, result
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "System bypass gaps found:\n{}",
        failures.join("\n")
    );
}

/// Documents the KNOWN gap: a no-rights actor currently PASSES every
/// `open_cells` matrix cell (Root/FunctionNamespace/User/Group/absent-
/// Function default to `ResourceMeta::open()`). This is the flip side of
/// `access_hierarchy_matrix_denies_no_rights_actor` above — it exists so
/// that when task #550 ("Root/User/Group always-open meta") closes one of
/// these, THIS assertion starts failing (a cell that used to be `Ok` is
/// now `Err`), forcing that fix to also move the cell from `open_cells`
/// into `x_cells` here rather than leaving the coverage matrix silently
/// out of sync with the enforcement it's supposed to document.
#[tokio::test]
async fn access_hierarchy_matrix_open_cells_are_currently_unenforced() {
    use shamir_types::access::Actor;

    let shamir = setup().await;
    let stranger = Actor::User(999_999);

    let cells = open_cells();
    let mut unexpectedly_denied = Vec::new();
    for cell in &cells {
        let path = (cell.path)();
        let result = shamir.authorize_access(&stranger, &path, cell.action).await;
        if result.is_err() {
            unexpectedly_denied.push(format!(
                "{}: expected this cell to still be open-by-design (Ok), got {:?} — \
                 if this is intentional (task #550 closed the gap), move this cell \
                 from `open_cells` to `x_cells` in this file",
                cell.label, result
            ));
        }
    }

    assert!(
        unexpectedly_denied.is_empty(),
        "open-by-design cells started denying (update coverage_matrix_tests.rs to match \
         the new enforcement):\n{}",
        unexpectedly_denied.join("\n")
    );
}
