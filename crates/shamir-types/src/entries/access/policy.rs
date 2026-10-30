use super::{AccessError, Action, Actor, Mode, Perm, PermClass, ResourceMeta, ResourcePath};

/// Map an [`Action`] to the [`Perm`] bit it checks in the mode word.
///
/// `Read` / `List` → `Read`; `Write` / `Create` / `Delete` → `Write`;
/// `Execute` → `Execute`. `Manage` is not a mode bit; it is owner-or-admin
/// only and is handled separately in [`permits`].
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
    match action_perm(action) {
        Some(perm) => Mode::is_set(meta.mode, class_of(actor, meta, in_group), perm),
        None => false,
    }
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
/// already run. A call here is not a reason to remove that outer gate.
/// New gates must call [`ShamirDb::authorize_access`] or wire through
/// `required_access`, never this function alone.
///
/// This function was named `authorize` until the #546 hardening pass;
/// it was renamed to make this no-op trace role unmistakable at call sites.
pub fn trace_access(actor: &Actor, path: &ResourcePath, action: Action) -> Result<(), AccessError> {
    log::trace!(
        "shomer: {actor} {action} on {}",
        super::escape_resource_diagnostic(&path.to_string())
    );
    Ok(())
}
