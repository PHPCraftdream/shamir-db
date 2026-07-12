//! Shomer access-control primitive types.
//!
//! These types model *who* is acting ([`Actor`]), *what* they target
//! ([`ResourcePath`]), and *how* ([`Action`]). [`trace_access`] is the
//! engine-level OBSERVABILITY trace (always `Ok` — not an enforcement
//! gate). Real enforcement lives in [`permits`] (pure decision) and the
//! facade gate in `shamir-db` (meta resolution + ancestor traversal).
//!
//! The full object & operation hierarchy is specified in
//! `docs/roadmap/ACCESS_HIERARCHY.md`.

use std::fmt;

use crate::types::common::new_map;
use crate::types::value::QueryValue;

/// Reserved u64 id for the `System` actor in persisted owner fields.
///
/// `Actor::System` serialises to `OWNER_SYSTEM` in catalogue records;
/// `Actor::User(id)` serialises to the user's numeric id.
pub const OWNER_SYSTEM: u64 = 0;

/// Fixed 63-bit projection of a directory-minted 16-byte user id into the
/// catalogue's `i64`-safe integer space (owner / group-member encoding).
///
/// Pure truncation — NOT a hash of anything attacker-chosen. Uniqueness and
/// non-zero-ness are enforced once, at mint time, by the directory that
/// produces the 16 bytes (`FjallUserDirectory`, task #556) — this function
/// only projects. See `docs/design/identity-privilege-unification-548-549-decision.md`
/// §2.2 for the full rationale (why 63-bit projection over widening the
/// catalogue to 128 bits).
pub fn principal64(user_id: [u8; 16]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&user_id[0..8]);
    u64::from_be_bytes(buf) & (i64::MAX as u64)
}

/// Interim, username-keyed bridge into the `principal64` id space.
///
/// **NOT the real identity primitive** — it is a deterministic hash of a
/// *name*, exactly what `principal64`'s real 16-byte projection is designed
/// to replace (see `principal_id`'s deletion, this same commit). It exists
/// ONLY for two classes of caller that cannot reach a real 16-byte
/// directory-minted id today:
///
/// 1. Two production call sites in `shamir-db`'s `access_control.rs` that
///    resolve a synthetic `/users/<name>` resource path (or list existing
///    usernames for `access_tree`) with no live `Session`/directory lookup
///    available — both are replaced by `PrincipalResolver` (task #559),
///    which resolves a REAL principal64 id via the directory. Do not add
///    new production call sites of this function; if you need a principal
///    id in new production code, you have a `Session` (use
///    `principal64(session.user_id)`) or you need `PrincipalResolver`.
/// 2. Test/bench fixtures that need a stable, per-name, non-colliding id
///    and do not care about mint-time randomness (they are not testing the
///    recreate-inherits-identity bug — that property is tested against
///    real `Session`/`user_id` bytes instead, see this task's red tests).
pub fn principal64_from_username(username: &str) -> u64 {
    let hash = fxhash::hash64(username);
    let mut user_id = [0u8; 16];
    user_id[0..8].copy_from_slice(&hash.to_be_bytes());
    principal64(user_id)
}

impl Actor {
    /// Persist-friendly u64 encoding: `System` → [`OWNER_SYSTEM`],
    /// `Admin(id)`/`User(id)` → `id`.
    pub fn to_owner_id(&self) -> u64 {
        match self {
            Actor::System => OWNER_SYSTEM,
            Actor::Admin(id) => *id,
            Actor::User(id) => *id,
        }
    }

    /// Decode a persisted owner id back into an [`Actor`].
    pub fn from_owner_id(id: u64) -> Self {
        if id == OWNER_SYSTEM {
            Actor::System
        } else {
            Actor::User(id)
        }
    }
}

// ── POSIX-style mode bits ────────────────────────────────────────────

/// Permission class for mode-bit queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermClass {
    Owner,
    Group,
    Other,
}

/// Permission bit (read / write / execute).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perm {
    Read,
    Write,
    Execute,
}

/// POSIX-style 12-bit mode helpers (`rwxrwxrwx` + setuid/setgid/sticky).
///
/// Bit layout (same as Unix `mode_t`, low 12 bits):
///
/// ```text
/// 11 10  9  8  7  6  5  4  3  2  1  0
/// s  s  t  rwx rwx rwx
/// │  │  │  │   │   └── other: r(2) w(1) x(0)
/// │  │  │  │   └────── group: r(5) w(4) x(3)
/// │  │  │  └────────── owner:  r(8) w(7) x(6)
/// │  │  └──────────────── sticky
/// │  └─────────────────── setgid
/// └────────────────────── setuid (0o4000)
/// ```
pub struct Mode;

/// setuid bit (bit 11, `0o4000`).
pub const MODE_SETUID: u16 = 0o4000;

impl Mode {
    /// Bit positions for r/w/x within a 3-bit class slot.
    const R: u16 = 0o4;
    const W: u16 = 0o2;
    const X: u16 = 0o1;

    /// Full rwx for one class.
    pub const RWX: u16 = Self::R | Self::W | Self::X;

    /// Open mode: owner/group/other all rwx (`0o777`).
    pub const OPEN: u16 = 0o777;

    /// Shift amount for each permission class.
    fn shift(class: PermClass) -> u16 {
        match class {
            PermClass::Other => 0,
            PermClass::Group => 3,
            PermClass::Owner => 6,
        }
    }

    /// Build a combined mode from per-class rwx flags.
    pub fn from_rwx(owner: bool, group: bool, other: bool) -> u16 {
        let mut mode: u16 = 0;
        if owner {
            mode |= Self::RWX << Self::shift(PermClass::Owner);
        }
        if group {
            mode |= Self::RWX << Self::shift(PermClass::Group);
        }
        if other {
            mode |= Self::RWX << Self::shift(PermClass::Other);
        }
        mode
    }

    /// Check whether a specific permission bit is set for a class.
    pub fn is_set(mode: u16, class: PermClass, perm: Perm) -> bool {
        let bit = match perm {
            Perm::Read => Self::R,
            Perm::Write => Self::W,
            Perm::Execute => Self::X,
        };
        (mode >> Self::shift(class)) & bit == bit
    }

    /// Check the setuid flag (bit 11).
    pub fn is_setuid(mode: u16) -> bool {
        mode & MODE_SETUID != 0
    }

    /// Set or clear the setuid flag.
    pub fn with_setuid(mode: u16, set: bool) -> u16 {
        if set {
            mode | MODE_SETUID
        } else {
            mode & !MODE_SETUID
        }
    }
}

// ── ResourceMeta ─────────────────────────────────────────────────────

/// Per-resource POSIX-style metadata envelope: owner, group, mode.
///
/// Mode-bearing objects (Database, Store, Table, Function, FunctionNamespace,
/// User, Group) each carry one of these. Record/Index inherit their Table's
/// meta. Default is [`ResourceMeta::open`] — System-owned, no group,
/// `0o777` (everyone rwx) — so nothing is restricted while the gate is
/// transparent and after P4 with open defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceMeta {
    pub owner: Actor,
    pub group: Option<u64>,
    pub mode: u16,
}

impl ResourceMeta {
    /// Open default: owner = System, group = None, mode = `0o777`.
    ///
    /// Every new catalogue record starts here; existing records without
    /// owner/group/mode fields load as this via [`ResourceMeta::from_record`].
    pub fn open() -> Self {
        Self {
            owner: Actor::System,
            group: None,
            mode: Mode::OPEN,
        }
    }

    /// Open-mode meta owned by the given actor.
    ///
    /// Same as [`open`](Self::open) but stamps the real creator as owner
    /// instead of `System`. Group stays `None`, mode stays `0o777` — no
    /// enforcement change, only ownership attribution.
    pub fn owned_by(actor: Actor) -> Self {
        Self {
            owner: actor,
            group: None,
            mode: Mode::OPEN,
        }
    }

    /// Enforced default: owner = actor, group = None, mode = owner-rwx (`0o700`).
    ///
    /// New mode-bearing objects are private to their creator (Strategy A).
    /// Legacy catalogue records without a `mode` field still load as
    /// [`open`](Self::open) via [`from_record`], so only NEW objects are
    /// enforced — existing data is unaffected.
    pub fn owned_enforced(actor: Actor) -> Self {
        Self {
            owner: actor,
            group: None,
            mode: Mode::from_rwx(true, false, false),
        }
    }

    /// Inject `owner`/`group`/`mode` fields into a `QueryValue::Map`
    /// catalogue record for persistence.
    pub fn inject_into(&self, rec: &mut QueryValue) {
        if let QueryValue::Map(map) = rec {
            map.insert(
                "owner".to_string(),
                // Store as i64: owner id is masked to i64::MAX in principal64.
                QueryValue::Int(self.owner.to_owner_id() as i64),
            );
            map.insert(
                "group".to_string(),
                match self.group {
                    Some(gid) => QueryValue::Int(gid as i64),
                    None => QueryValue::Null,
                },
            );
            map.insert("mode".to_string(), QueryValue::Int(self.mode as i64));
        }
    }

    /// Decode `owner`/`group`/`mode` from a persisted `QueryValue` catalogue
    /// record.
    ///
    /// Backward-compatible: records that lack any of the three fields fall
    /// back to [`ResourceMeta::open`] defaults.
    pub fn from_record(rec: &QueryValue) -> Self {
        let owner = rec
            .get("owner")
            .and_then(|v| v.as_u64())
            .map(Actor::from_owner_id)
            .unwrap_or(Actor::System);
        let group = rec.get("group").and_then(|v| v.as_u64());
        let mode = rec
            .get("mode")
            .and_then(|v| v.as_u64())
            .and_then(|m| u16::try_from(m).ok())
            .unwrap_or(Mode::OPEN);
        Self { owner, group, mode }
    }

    /// Returns `Some(owner)` iff the record has an explicit `owner` field;
    /// `None` if the field is absent.
    ///
    /// Distinct from [`from_record`](Self::from_record), which collapses
    /// "field absent" into `Actor::System` — the correct, deliberate default
    /// for every OTHER caller (DDL introspection, `access_tree`,
    /// `resource_meta`, etc., all of which treat a fresh/legacy record with
    /// no owner as "open, owned by System"). That collapse is wrong for a
    /// caller that uses the owner to make a privilege-ESCALATION decision
    /// (e.g. `ShamirDb::effective_fn_actor`): such a caller must be able to
    /// tell "explicitly owned by System" (owner field present, value `0`)
    /// apart from "no owner recorded at all" (field absent) — the former is
    /// a legitimate admin declaration, the latter must never escalate.
    pub fn owner_field(rec: &QueryValue) -> Option<Actor> {
        rec.get("owner")
            .and_then(|v| v.as_u64())
            .map(Actor::from_owner_id)
    }

    /// Build a `QueryValue::Map` containing only the `owner`/`group`/`mode`
    /// fields. Convenience for constructing fresh catalogue records.
    pub fn to_query_value(&self) -> QueryValue {
        let mut map = new_map();
        map.insert(
            "owner".to_string(),
            QueryValue::Int(self.owner.to_owner_id() as i64),
        );
        map.insert(
            "group".to_string(),
            match self.group {
                Some(gid) => QueryValue::Int(gid as i64),
                None => QueryValue::Null,
            },
        );
        map.insert("mode".to_string(), QueryValue::Int(self.mode as i64));
        QueryValue::Map(map)
    }
}

impl Default for ResourceMeta {
    fn default() -> Self {
        Self::open()
    }
}

/// The identity performing an operation.
///
/// `System` is the all-bypassing default used while the authentication
/// wire path is not yet plumbed through.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Actor {
    #[default]
    System,
    /// A superuser session. Bypasses `permits()` exactly like `System`, but
    /// (unlike `System`) carries the real principal64 id, so
    /// `ResourceMeta::owned_enforced`/`to_owner_id()` attributes
    /// admin-created resources to their creator instead of collapsing them
    /// to `owner = 0 = System`. NEVER produced by `from_owner_id` — admin-ness
    /// is a live session property, never a persisted owner property; a
    /// persisted owner id round-trips to `Actor::User`, never `Actor::Admin`,
    /// even if that id happens to belong to an admin account.
    Admin(u64),
    User(u64),
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::System => f.write_str("System"),
            Actor::Admin(id) => write!(f, "Admin({id})"),
            Actor::User(id) => write!(f, "User({id})"),
        }
    }
}

/// Uniform, traversable address of a securable resource in the tree.
///
/// The tree (see `ACCESS_HIERARCHY.md`):
/// ```text
/// Root
/// ├── databases/<db>/<store>/<table>/{<record>, indexes/<index>}
/// ├── functions/<function>            (FunctionNamespace → Function)
/// ├── users/<user>
/// └── groups/<group>
/// ```
/// [`parent`](Self::parent) walks toward the root so the gate can require
/// traversal (`Execute`) on every ancestor container.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourcePath {
    /// The system root — the admin domain.
    Root,
    Database {
        db: String,
    },
    Store {
        db: String,
        store: String,
    },
    Table {
        db: String,
        store: String,
        table: String,
    },
    /// A single row. Leaf; inherits its table's owner/mode (row-level
    /// metadata only when row-level security is enabled).
    Record {
        db: String,
        store: String,
        table: String,
        key: String,
    },
    /// A secondary index — derived; inherits its table.
    Index {
        db: String,
        store: String,
        table: String,
        index: String,
    },
    /// The container under which user-defined functions are created.
    FunctionNamespace,
    /// A folder within the function namespace, addressed by path segments.
    ///
    /// For example, the folder `reports/daily` is represented as
    /// `FunctionFolder { path: vec!["reports".into(), "daily".into()] }`.
    /// A function `reports/daily/orders` lives *inside* this folder.
    FunctionFolder {
        path: Vec<String>,
    },
    Function {
        name: String,
    },
    User {
        name: String,
    },
    Group {
        name: String,
    },
}

impl ResourcePath {
    /// Construct a database path.
    pub fn database(db: impl Into<String>) -> Self {
        ResourcePath::Database { db: db.into() }
    }
    /// Construct a store path.
    pub fn store(db: impl Into<String>, store: impl Into<String>) -> Self {
        ResourcePath::Store {
            db: db.into(),
            store: store.into(),
        }
    }
    /// Construct a table path.
    pub fn table(
        db: impl Into<String>,
        store: impl Into<String>,
        table: impl Into<String>,
    ) -> Self {
        ResourcePath::Table {
            db: db.into(),
            store: store.into(),
            table: table.into(),
        }
    }
    /// Construct a record path.
    pub fn record(
        db: impl Into<String>,
        store: impl Into<String>,
        table: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        ResourcePath::Record {
            db: db.into(),
            store: store.into(),
            table: table.into(),
            key: key.into(),
        }
    }
    /// Construct an index path.
    pub fn index(
        db: impl Into<String>,
        store: impl Into<String>,
        table: impl Into<String>,
        index: impl Into<String>,
    ) -> Self {
        ResourcePath::Index {
            db: db.into(),
            store: store.into(),
            table: table.into(),
            index: index.into(),
        }
    }
    /// Construct a function path.
    pub fn function(name: impl Into<String>) -> Self {
        ResourcePath::Function { name: name.into() }
    }
    /// Construct a function-folder path from path segments.
    ///
    /// An empty `segments` vec is treated as the function namespace root
    /// (callers should prefer [`FunctionNamespace`] directly).
    pub fn function_folder(segments: Vec<String>) -> Self {
        ResourcePath::FunctionFolder { path: segments }
    }
    /// Construct a user path.
    pub fn user(name: impl Into<String>) -> Self {
        ResourcePath::User { name: name.into() }
    }
    /// Construct a group path.
    pub fn group(name: impl Into<String>) -> Self {
        ResourcePath::Group { name: name.into() }
    }

    /// The containing resource, or `None` for the root.
    ///
    /// Record/Index resolve to their Table (inheritance); the top-level
    /// containers (Database, FunctionNamespace, User, Group) resolve to Root.
    pub fn parent(&self) -> Option<ResourcePath> {
        match self {
            ResourcePath::Root => None,
            ResourcePath::Database { .. } => Some(ResourcePath::Root),
            ResourcePath::Store { db, .. } => Some(ResourcePath::database(db.clone())),
            ResourcePath::Table { db, store, .. } => {
                Some(ResourcePath::store(db.clone(), store.clone()))
            }
            ResourcePath::Record {
                db, store, table, ..
            }
            | ResourcePath::Index {
                db, store, table, ..
            } => Some(ResourcePath::table(
                db.clone(),
                store.clone(),
                table.clone(),
            )),
            ResourcePath::FunctionNamespace => Some(ResourcePath::Root),
            ResourcePath::FunctionFolder { path } => {
                if path.len() > 1 {
                    Some(ResourcePath::function_folder(
                        path[..path.len() - 1].to_vec(),
                    ))
                } else {
                    // Single-segment or empty folder → namespace root.
                    Some(ResourcePath::FunctionNamespace)
                }
            }
            ResourcePath::Function { name } => {
                if let Some(pos) = name.rfind('/') {
                    // Slash-qualified name: derive the folder from the prefix.
                    let segments: Vec<String> =
                        name[..pos].split('/').map(|s| s.to_string()).collect();
                    Some(ResourcePath::function_folder(segments))
                } else {
                    Some(ResourcePath::FunctionNamespace)
                }
            }
            ResourcePath::User { .. } | ResourcePath::Group { .. } => Some(ResourcePath::Root),
        }
    }

    /// Ancestor containers, nearest first, up to and including `Root`
    /// (excludes `self`). The gate requires `Execute` (traverse) on each.
    pub fn ancestors(&self) -> Vec<ResourcePath> {
        let mut out = Vec::with_capacity(4);
        let mut cur = self.parent();
        while let Some(p) = cur {
            cur = p.parent();
            out.push(p);
        }
        out
    }
}

impl fmt::Display for ResourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourcePath::Root => f.write_str("/"),
            ResourcePath::Database { db } => write!(f, "db://{db}"),
            ResourcePath::Store { db, store } => write!(f, "db://{db}/{store}"),
            ResourcePath::Table { db, store, table } => write!(f, "db://{db}/{store}/{table}"),
            ResourcePath::Record {
                db,
                store,
                table,
                key,
            } => write!(f, "db://{db}/{store}/{table}#{key}"),
            ResourcePath::Index {
                db,
                store,
                table,
                index,
            } => write!(f, "db://{db}/{store}/{table}.idx/{index}"),
            ResourcePath::FunctionNamespace => f.write_str("fn://"),
            ResourcePath::FunctionFolder { path } => write!(f, "fn://{}/", path.join("/")),
            ResourcePath::Function { name } => write!(f, "fn://{name}"),
            ResourcePath::User { name } => write!(f, "user://{name}"),
            ResourcePath::Group { name } => write!(f, "group://{name}"),
        }
    }
}

/// The class of operation being performed on a resource.
///
/// POSIX-flavoured: `Read`/`Write`/`Execute` map to `r`/`w`/`x`; `Create`
/// and `Delete` are writes on a container; `List` is read on a container;
/// `Manage` is the owner/admin-only class (chmod/chown/chgrp/grant).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Read,
    Write,
    Create,
    Delete,
    Execute,
    List,
    Manage,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Read => "READ",
            Action::Write => "WRITE",
            Action::Create => "CREATE",
            Action::Delete => "DELETE",
            Action::Execute => "EXECUTE",
            Action::List => "LIST",
            Action::Manage => "MANAGE",
        })
    }
}

/// Access denied (constructed by the real policy check in P4).
#[derive(Debug, thiserror::Error)]
#[error("access denied: {actor} cannot {action} on {path}")]
pub struct AccessError {
    pub actor: Actor,
    /// The rendered resource path (kept as a `String`, not the full
    /// `ResourcePath` enum, so the error stays small — `Result<_, AccessError>`
    /// is on the hot path and `clippy::result_large_err` would fire otherwise).
    pub path: String,
    pub action: Action,
}

/// Transparent OBSERVABILITY trace (engine-level, R2) — **NOT an
/// enforcement gate**.
///
/// Always returns `Ok(())` and emits a `log::trace!` access line. Despite
/// the historical name, a call to `trace_access` performs no access
/// check whatsoever — it exists purely so every attempted operation
/// leaves a trace line for observability. The real POSIX-style
/// enforcement happens in [`permits`] + the facade gate
/// (`ShamirDb::authorize_access`, which also re-emits this same trace
/// before doing the actual check).
///
/// Call sites in `shamir-engine`'s `QueryRunner::run` (the per-op
/// `Read`/`Insert`/`Update`/`Delete`/`Set` match arms) invoke this
/// AFTER the real gate (`execute_as`/`tx_execute_as`'s per-op
/// `authorize_access` loop, driven by `BatchOp::required_access`) has
/// already run — a future refactor must not see a call here, assume
/// it's the enforcement point, and remove the outer `authorize_access`
/// call thinking it's now redundant. If you are adding a NEW gate, call
/// [`ShamirDb::authorize_access`] (or wire through `required_access`),
/// never this function alone.
///
/// This function was named `authorize` until the #546 hardening pass —
/// renamed specifically to make "this is a no-op trace, not a check"
/// unmistakable from the call site, without needing to read this doc
/// comment.
pub fn trace_access(actor: &Actor, path: &ResourcePath, action: Action) -> Result<(), AccessError> {
    log::trace!("shomer: {actor} {action} on {path}");
    Ok(())
}

/// Map an [`Action`] to the [`Perm`] bit it checks in the mode word.
///
/// `Read` / `List` → `Read`; `Write` / `Create` / `Delete` → `Write`;
/// `Execute` → `Execute`. `Manage` is **not** a mode bit — it is
/// owner-or-admin only, handled separately in [`permits`].
pub const fn action_perm(action: Action) -> Option<Perm> {
    match action {
        Action::Read | Action::List => Some(Perm::Read),
        Action::Write | Action::Create | Action::Delete => Some(Perm::Write),
        Action::Execute => Some(Perm::Execute),
        Action::Manage => None,
    }
}

/// Determine the [`PermClass`] for an actor against a resource.
///
/// First match wins (POSIX semantics — NOT a union):
/// 1. Owner — if the actor's owner id matches the meta's owner.
/// 2. Group — if the meta has a group *and* `in_group` is true.
/// 3. Other — fallback.
pub fn class_of(actor: &Actor, meta: &ResourceMeta, in_group: bool) -> PermClass {
    if actor.to_owner_id() == meta.owner.to_owner_id() {
        PermClass::Owner
    } else if meta.group.is_some() && in_group {
        PermClass::Group
    } else {
        PermClass::Other
    }
}

/// Pure POSIX-style permission check (no catalogue dependency).
///
/// Callers supply the [`ResourceMeta`] and `in_group` flag; this function
/// only evaluates the mode bits. Returns `true` if the action is allowed.
///
/// * `Actor::System` / `Actor::Admin(_)` → always `true` (admin bypass).
/// * `Action::Manage` → `true` iff the actor is the owner (System/Admin already
///   returned above).
/// * Otherwise → pick [`class_of`], map the action to a [`Perm`] via
///   [`action_perm`], and check [`Mode::is_set`].
pub fn permits(actor: &Actor, meta: &ResourceMeta, action: Action, in_group: bool) -> bool {
    if matches!(actor, Actor::System | Actor::Admin(_)) {
        return true;
    }
    if action == Action::Manage {
        return actor.to_owner_id() == meta.owner.to_owner_id();
    }
    let class = class_of(actor, meta, in_group);
    let perm = match action_perm(action) {
        Some(p) => p,
        None => return false,
    };
    Mode::is_set(meta.mode, class, perm)
}
