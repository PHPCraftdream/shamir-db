//! Shomer access-control primitive types.
//!
//! These types model *who* is acting ([`Actor`]), *what* they target
//! ([`ResourcePath`]), and *how* ([`Action`]). The [`authorize`] gate is
//! the engine-level trace (always `Ok`). Real enforcement lives in
//! [`permits`] (pure decision) and the facade gate in `shamir-db`
//! (meta resolution + ancestor traversal).
//!
//! The full object & operation hierarchy is specified in
//! `docs/roadmap/ACCESS_HIERARCHY.md`.

use std::fmt;

/// Reserved u64 id for the `System` actor in persisted owner fields.
///
/// `Actor::System` serialises to `OWNER_SYSTEM` in catalogue records;
/// `Actor::User(id)` serialises to the user's numeric id.
pub const OWNER_SYSTEM: u64 = 0;

/// Canonical principal id for a username.
///
/// Hashes the username with fxhash and masks to 63 bits so the id always
/// fits an `i64`: the catalogue stores integers as `i64` (owner /
/// group-member ids round-trip through JSON→InnerValue→msgpack), and a
/// `u64` above `i64::MAX` would be lost on read-back. The wire session
/// layer and the access-tree resolver both call this, so an owner id on a
/// resource resolves back to the same username everywhere. The reserved
/// `System` actor keeps id `0` ([`OWNER_SYSTEM`]); a username hashing to
/// `0` is astronomically unlikely and would merely alias the system id.
pub fn principal_id(username: &str) -> u64 {
    fxhash::hash64(username) & (i64::MAX as u64)
}

impl Actor {
    /// Persist-friendly u64 encoding: `System` → [`OWNER_SYSTEM`],
    /// `User(id)` → `id`.
    pub fn to_owner_id(&self) -> u64 {
        match self {
            Actor::System => OWNER_SYSTEM,
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

    /// Inject `owner`/`group`/`mode` fields into a JSON catalogue record
    /// for persistence. Safe to call on any `serde_json::Value::Object`.
    pub fn inject_into(&self, rec: &mut serde_json::Value) {
        if let Some(map) = rec.as_object_mut() {
            map.insert(
                "owner".to_string(),
                serde_json::Value::Number(self.owner.to_owner_id().into()),
            );
            map.insert(
                "group".to_string(),
                match self.group {
                    Some(gid) => serde_json::Value::Number(gid.into()),
                    None => serde_json::Value::Null,
                },
            );
            map.insert(
                "mode".to_string(),
                serde_json::Value::Number(self.mode.into()),
            );
        }
    }

    /// Decode `owner`/`group`/`mode` from a persisted JSON catalogue record.
    ///
    /// Backward-compatible: records that lack any of the three fields fall
    /// back to [`ResourceMeta::open`] defaults.
    pub fn from_record(rec: &serde_json::Value) -> Self {
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
    User(u64),
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::System => f.write_str("System"),
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
        let mut out = Vec::new();
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Transparent authorization gate (engine-level trace, R2).
///
/// Always returns `Ok(())` and emits a `log::trace!` access line. The real
/// POSIX-style enforcement happens in [`permits`] + the facade gate.
pub fn authorize(actor: &Actor, path: &ResourcePath, action: Action) -> Result<(), AccessError> {
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
/// * `Actor::System` → always `true` (admin bypass).
/// * `Action::Manage` → `true` iff the actor is the owner (System already
///   returned above).
/// * Otherwise → pick [`class_of`], map the action to a [`Perm`] via
///   [`action_perm`], and check [`Mode::is_set`].
pub fn permits(actor: &Actor, meta: &ResourceMeta, action: Action, in_group: bool) -> bool {
    if matches!(actor, Actor::System) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_is_default() {
        assert_eq!(Actor::default(), Actor::System);
    }

    #[test]
    fn authorize_transparent_for_all_variants() {
        for path in [
            ResourcePath::Root,
            ResourcePath::database("d"),
            ResourcePath::store("d", "s"),
            ResourcePath::table("d", "s", "t"),
            ResourcePath::record("d", "s", "t", "k"),
            ResourcePath::index("d", "s", "t", "i"),
            ResourcePath::FunctionNamespace,
            ResourcePath::function_folder(vec!["reports".to_string()]),
            ResourcePath::function("f"),
            ResourcePath::user("u"),
            ResourcePath::group("g"),
        ] {
            for action in [
                Action::Read,
                Action::Write,
                Action::Create,
                Action::Delete,
                Action::Execute,
                Action::List,
                Action::Manage,
            ] {
                assert!(authorize(&Actor::System, &path, action).is_ok());
            }
        }
    }

    #[test]
    fn parent_walks_to_root() {
        // record → table → store → database → root → None
        let rec = ResourcePath::record("d", "s", "t", "k");
        let table = rec.parent().unwrap();
        assert_eq!(table, ResourcePath::table("d", "s", "t"));
        let store = table.parent().unwrap();
        assert_eq!(store, ResourcePath::store("d", "s"));
        let db = store.parent().unwrap();
        assert_eq!(db, ResourcePath::database("d"));
        let root = db.parent().unwrap();
        assert_eq!(root, ResourcePath::Root);
        assert_eq!(root.parent(), None);
    }

    #[test]
    fn index_inherits_table_as_parent() {
        let idx = ResourcePath::index("d", "s", "t", "i");
        assert_eq!(idx.parent().unwrap(), ResourcePath::table("d", "s", "t"));
    }

    #[test]
    fn function_parent_is_namespace() {
        assert_eq!(
            ResourcePath::function("f").parent().unwrap(),
            ResourcePath::FunctionNamespace
        );
        assert_eq!(
            ResourcePath::FunctionNamespace.parent().unwrap(),
            ResourcePath::Root
        );
    }

    #[test]
    fn ancestors_nearest_first_to_root() {
        let rec = ResourcePath::record("d", "s", "t", "k");
        assert_eq!(
            rec.ancestors(),
            vec![
                ResourcePath::table("d", "s", "t"),
                ResourcePath::store("d", "s"),
                ResourcePath::database("d"),
                ResourcePath::Root,
            ]
        );
    }

    #[test]
    fn user_and_group_under_root() {
        assert_eq!(
            ResourcePath::user("u").parent().unwrap(),
            ResourcePath::Root
        );
        assert_eq!(
            ResourcePath::group("g").parent().unwrap(),
            ResourcePath::Root
        );
    }

    // ========================================================================
    // ResourceMeta + Mode tests
    // ========================================================================

    #[test]
    fn open_default_is_system_owned_mode_777() {
        let open = ResourceMeta::open();
        assert_eq!(open.owner, Actor::System);
        assert!(open.group.is_none());
        assert_eq!(open.mode, 0o777);
        assert_eq!(ResourceMeta::default(), open);
    }

    #[test]
    fn actor_owner_round_trip() {
        assert_eq!(Actor::System.to_owner_id(), OWNER_SYSTEM);
        assert_eq!(Actor::from_owner_id(OWNER_SYSTEM), Actor::System);
        assert_eq!(Actor::User(42).to_owner_id(), 42);
        assert_eq!(Actor::from_owner_id(42), Actor::User(42));
    }

    #[test]
    fn principal_id_is_deterministic_and_distinct() {
        // Stable across calls for the same name.
        assert_eq!(principal_id("alice"), principal_id("alice"));
        // Distinct names hash apart (no trivial collision for these).
        assert_ne!(principal_id("alice"), principal_id("bob"));
    }

    #[test]
    fn principal_id_always_fits_i64() {
        // The catalogue stores ids as i64; every principal id must be
        // <= i64::MAX so it survives the JSON→InnerValue→msgpack round-trip
        // (the root cause of the empty group-member bug in HIGH-7).
        for name in [
            "",
            "a",
            "admin",
            "alice",
            "bob",
            "Σίσυφος",
            "очень-длинное-имя-пользователя-1234567890",
        ] {
            assert!(
                principal_id(name) <= i64::MAX as u64,
                "principal_id({name:?}) overflowed i64"
            );
        }
    }

    #[test]
    fn principal_id_round_trips_through_actor_owner_id() {
        // A user id derived from a name must decode back to the same
        // `Actor::User`, never aliasing the reserved System id.
        let id = principal_id("alice");
        assert_ne!(id, OWNER_SYSTEM);
        assert_eq!(Actor::from_owner_id(id), Actor::User(id));
    }

    #[test]
    fn mode_from_rwx_combinations() {
        assert_eq!(Mode::from_rwx(true, true, true), 0o777);
        assert_eq!(Mode::from_rwx(true, false, false), 0o700);
        assert_eq!(Mode::from_rwx(false, true, false), 0o070);
        assert_eq!(Mode::from_rwx(false, false, true), 0o007);
        assert_eq!(Mode::from_rwx(false, false, false), 0o000);
    }

    #[test]
    fn mode_is_set_checks() {
        // 0o770 = rwxrwx---
        let mode = 0o770;
        assert!(Mode::is_set(mode, PermClass::Owner, Perm::Read));
        assert!(Mode::is_set(mode, PermClass::Owner, Perm::Write));
        assert!(Mode::is_set(mode, PermClass::Owner, Perm::Execute));
        assert!(Mode::is_set(mode, PermClass::Group, Perm::Read));
        assert!(Mode::is_set(mode, PermClass::Group, Perm::Write));
        assert!(Mode::is_set(mode, PermClass::Group, Perm::Execute));
        assert!(!Mode::is_set(mode, PermClass::Other, Perm::Read));
        assert!(!Mode::is_set(mode, PermClass::Other, Perm::Write));
        assert!(!Mode::is_set(mode, PermClass::Other, Perm::Execute));
        // 0o750 = rwxr-x---
        let mode2 = 0o750;
        assert!(!Mode::is_set(mode2, PermClass::Group, Perm::Write));
        assert!(Mode::is_set(mode2, PermClass::Group, Perm::Execute));
    }

    #[test]
    fn setuid_flag_accessor() {
        let mode = 0o777;
        assert!(!Mode::is_setuid(mode));
        let mode_suid = Mode::with_setuid(mode, true);
        assert!(Mode::is_setuid(mode_suid));
        assert_eq!(mode_suid & 0o777, 0o777);
        let cleared = Mode::with_setuid(mode_suid, false);
        assert!(!Mode::is_setuid(cleared));
        assert_eq!(cleared, 0o777);
    }

    #[test]
    fn inject_into_and_from_record_round_trip() {
        let meta = ResourceMeta {
            owner: Actor::User(10),
            group: Some(5),
            mode: 0o750,
        };
        let mut rec = serde_json::json!({"name": "test"});
        meta.inject_into(&mut rec);
        assert_eq!(rec["owner"], 10);
        assert_eq!(rec["group"], 5);
        assert_eq!(rec["mode"], 0o750);

        let loaded = ResourceMeta::from_record(&rec);
        assert_eq!(loaded.owner, Actor::User(10));
        assert_eq!(loaded.group, Some(5));
        assert_eq!(loaded.mode, 0o750);
    }

    #[test]
    fn from_record_backward_compat_returns_open() {
        let rec = serde_json::json!({"name": "legacy"});
        let loaded = ResourceMeta::from_record(&rec);
        assert_eq!(loaded, ResourceMeta::open());
    }

    #[test]
    fn from_record_null_group_is_none() {
        let rec = serde_json::json!({
            "name": "test",
            "owner": 42,
            "group": null,
            "mode": 0o644,
        });
        let loaded = ResourceMeta::from_record(&rec);
        assert_eq!(loaded.owner, Actor::User(42));
        assert!(loaded.group.is_none());
        assert_eq!(loaded.mode, 0o644);
    }

    // ========================================================================
    // permits / class_of tests (P4)
    // ========================================================================

    #[test]
    fn action_perm_mapping() {
        assert_eq!(action_perm(Action::Read), Some(Perm::Read));
        assert_eq!(action_perm(Action::List), Some(Perm::Read));
        assert_eq!(action_perm(Action::Write), Some(Perm::Write));
        assert_eq!(action_perm(Action::Create), Some(Perm::Write));
        assert_eq!(action_perm(Action::Delete), Some(Perm::Write));
        assert_eq!(action_perm(Action::Execute), Some(Perm::Execute));
        assert_eq!(action_perm(Action::Manage), None);
    }

    #[test]
    fn class_of_owner_first_match() {
        let meta = ResourceMeta {
            owner: Actor::User(10),
            group: Some(5),
            mode: 0o777,
        };
        // Owner matches even though group is present and in_group is true.
        assert_eq!(class_of(&Actor::User(10), &meta, true), PermClass::Owner);
        // Not the owner but in group.
        assert_eq!(class_of(&Actor::User(20), &meta, true), PermClass::Group);
        // Not the owner, not in group.
        assert_eq!(class_of(&Actor::User(20), &meta, false), PermClass::Other);
        // Group is None — no group class even if in_group is true.
        let no_group = ResourceMeta {
            owner: Actor::User(10),
            group: None,
            mode: 0o777,
        };
        assert_eq!(
            class_of(&Actor::User(20), &no_group, true),
            PermClass::Other
        );
    }

    #[test]
    fn permits_system_bypass() {
        let meta = ResourceMeta {
            owner: Actor::User(99),
            group: None,
            mode: 0o000,
        };
        for action in [
            Action::Read,
            Action::Write,
            Action::Create,
            Action::Delete,
            Action::Execute,
            Action::List,
            Action::Manage,
        ] {
            assert!(
                permits(&Actor::System, &meta, action, false),
                "System should bypass for {action}"
            );
        }
    }

    #[test]
    fn permits_owner_class_rwx() {
        let meta = ResourceMeta {
            owner: Actor::User(10),
            group: None,
            mode: 0o700,
        };
        assert!(permits(&Actor::User(10), &meta, Action::Read, false));
        assert!(permits(&Actor::User(10), &meta, Action::Write, false));
        assert!(permits(&Actor::User(10), &meta, Action::Execute, false));
        assert!(permits(&Actor::User(10), &meta, Action::Create, false));
        assert!(permits(&Actor::User(10), &meta, Action::Delete, false));
        assert!(permits(&Actor::User(10), &meta, Action::List, false));
        assert!(permits(&Actor::User(10), &meta, Action::Manage, false));
    }

    #[test]
    fn permits_group_class() {
        let meta = ResourceMeta {
            owner: Actor::User(10),
            group: Some(5),
            mode: 0o070,
        };
        // In group → allowed (group bits are rwx).
        assert!(permits(&Actor::User(20), &meta, Action::Read, true));
        assert!(permits(&Actor::User(20), &meta, Action::Write, true));
        assert!(permits(&Actor::User(20), &meta, Action::Execute, true));
        // Not in group → denied (owner has rwx but actor is not owner;
        // other bits are 0).
        assert!(!permits(&Actor::User(20), &meta, Action::Read, false));
        assert!(!permits(&Actor::User(20), &meta, Action::Write, false));
    }

    #[test]
    fn permits_other_class() {
        let meta = ResourceMeta {
            owner: Actor::User(10),
            group: None,
            mode: 0o007,
        };
        assert!(permits(&Actor::User(20), &meta, Action::Read, false));
        assert!(permits(&Actor::User(20), &meta, Action::Write, false));
        assert!(permits(&Actor::User(20), &meta, Action::Execute, false));
    }

    #[test]
    fn permits_manage_owner_only() {
        let meta = ResourceMeta {
            owner: Actor::User(10),
            group: Some(5),
            mode: 0o777,
        };
        // Owner can manage.
        assert!(permits(&Actor::User(10), &meta, Action::Manage, false));
        // Non-owner cannot manage, even with full mode bits.
        assert!(!permits(&Actor::User(20), &meta, Action::Manage, true));
        assert!(!permits(&Actor::User(20), &meta, Action::Manage, false));
    }

    #[test]
    fn permits_first_match_wins_owner_denied() {
        // Owner bits are 0, but other bits are rwx. POSIX first-match:
        // actor IS the owner → Owner class → owner bits (0) → denied.
        let meta = ResourceMeta {
            owner: Actor::User(10),
            group: None,
            mode: 0o007,
        };
        assert!(
            !permits(&Actor::User(10), &meta, Action::Read, false),
            "owner should be denied when owner bits are 0 despite other=rwx"
        );
    }

    #[test]
    fn permits_open_mode_allows_everyone() {
        let meta = ResourceMeta::open(); // mode 0o777
        assert!(permits(&Actor::User(99), &meta, Action::Read, false));
        assert!(permits(&Actor::User(99), &meta, Action::Write, false));
        assert!(permits(&Actor::User(99), &meta, Action::Execute, false));
    }

    // ========================================================================
    // FunctionFolder tests (#118)
    // ========================================================================

    #[test]
    fn function_folder_parent_chain_for_folder_qualified_function() {
        // reports/daily/orders → folder ["reports","daily"] → ["reports"] → FunctionNamespace → Root
        let func = ResourcePath::function("reports/daily/orders");
        let folder2 = func.parent().unwrap();
        assert_eq!(
            folder2,
            ResourcePath::function_folder(vec!["reports".to_string(), "daily".to_string(),])
        );
        let folder1 = folder2.parent().unwrap();
        assert_eq!(
            folder1,
            ResourcePath::function_folder(vec!["reports".to_string()])
        );
        let ns = folder1.parent().unwrap();
        assert_eq!(ns, ResourcePath::FunctionNamespace);
        let root = ns.parent().unwrap();
        assert_eq!(root, ResourcePath::Root);
        assert_eq!(root.parent(), None);
    }

    #[test]
    fn single_segment_function_parent_is_namespace() {
        // A function without `/` has FunctionNamespace as parent (unchanged).
        let func = ResourcePath::function("my_fn");
        assert_eq!(func.parent().unwrap(), ResourcePath::FunctionNamespace);
    }

    #[test]
    fn function_folder_ancestors_order() {
        let func = ResourcePath::function("reports/daily/orders");
        let ancestors = func.ancestors();
        assert_eq!(
            ancestors,
            vec![
                ResourcePath::function_folder(vec!["reports".to_string(), "daily".to_string(),]),
                ResourcePath::function_folder(vec!["reports".to_string()]),
                ResourcePath::FunctionNamespace,
                ResourcePath::Root,
            ]
        );
    }

    #[test]
    fn function_folder_display() {
        let folder =
            ResourcePath::function_folder(vec!["reports".to_string(), "daily".to_string()]);
        assert_eq!(folder.to_string(), "fn://reports/daily/");

        let single = ResourcePath::function_folder(vec!["utils".to_string()]);
        assert_eq!(single.to_string(), "fn://utils/");
    }

    #[test]
    fn function_folder_constructor() {
        let folder = ResourcePath::function_folder(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            folder,
            ResourcePath::FunctionFolder {
                path: vec!["a".to_string(), "b".to_string()],
            }
        );
    }

    #[test]
    fn empty_function_folder_parent_is_namespace() {
        let folder = ResourcePath::function_folder(vec![]);
        assert_eq!(folder.parent().unwrap(), ResourcePath::FunctionNamespace);
    }
}
