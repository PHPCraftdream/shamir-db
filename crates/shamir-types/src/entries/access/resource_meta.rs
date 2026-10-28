use crate::types::common::new_map;
use crate::types::value::QueryValue;

use super::{Actor, Mode};

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
    /// Legacy records without a mode still load as open; only NEW objects are
    /// enforced, so existing data is unaffected.
    pub fn owned_enforced(actor: Actor) -> Self {
        Self {
            owner: actor,
            group: None,
            mode: Mode::from_rwx(true, false, false),
        }
    }

    /// Inject `owner`/`group`/`mode` fields into a map catalogue record.
    pub fn inject_into(&self, rec: &mut QueryValue) {
        if let QueryValue::Map(map) = rec {
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
        }
    }

    /// Decode `owner`/`group`/`mode` from persisted catalogue metadata.
    ///
    /// Backward-compatible when a field is absent. Missing mode preserves the
    /// legacy open default; a malformed present mode fails closed to no rights.
    pub fn from_record(rec: &QueryValue) -> Self {
        let owner = rec
            .get("owner")
            .and_then(|v| v.as_u64())
            .map(Actor::from_owner_id)
            .unwrap_or(Actor::System);
        let group = rec.get("group").and_then(|v| v.as_u64());
        let mode = match rec.get("mode") {
            None => Mode::OPEN,
            Some(value) => value
                .as_u64()
                .and_then(|m| u16::try_from(m).ok())
                .unwrap_or(0),
        };
        Self { owner, group, mode }
    }

    /// Returns Some(owner) iff the record has an explicit valid owner field;
    /// None if the field is absent.
    ///
    /// Distinct from [`from_record`](Self::from_record), which collapses
    /// absent owner into System for ordinary open-record callers. A caller
    /// making an escalation decision must distinguish absent ownership from
    /// explicit System ownership (owner field present with value zero).
    pub fn owner_field(rec: &QueryValue) -> Option<Actor> {
        rec.get("owner")
            .and_then(|v| v.as_u64())
            .map(Actor::from_owner_id)
    }

    /// Build a QueryValue map containing only owner/group/mode fields.
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
