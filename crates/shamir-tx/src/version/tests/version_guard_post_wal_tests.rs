use crate::RepoTxGate;

#[test]
fn pre_wal_drop_completes_both_trackers_as_aborted() {
    let gate = RepoTxGate::fresh();
    let guard = gate.assign_next_version_guarded();
    let version = guard.version();
    drop(guard);
    assert_eq!(gate.completion().watermark(), version);
    assert_eq!(gate.last_committed(), version);
    assert_eq!(gate.durable_watermark(), version);
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(gate.orphan_max(), 0);
}

#[test]
fn post_wal_drop_registers_only_orphan_bound_until_completion() {
    let gate = RepoTxGate::fresh();
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    guard.wal_accepted();
    guard.wal_accepted();
    assert!(gate.is_active_post_wal(version));
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(gate.orphan_max(), 0);
    drop(guard);
    assert!(!gate.is_active_post_wal(version));
    assert!(gate.is_orphaned_post_wal(version));
    assert_eq!(gate.orphan_max(), version);
    assert_eq!(gate.completion().watermark(), 0);
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(gate.durable_watermark(), 0);
    assert!(gate.orphan_entry(version).is_none());
    assert!(gate.unresolved_wal_versions().is_empty());
    assert!(!gate.requires_reopen());

    gate.assign_next_version_guarded().commit();
    assert_eq!(gate.completion().watermark(), 0);
    gate.complete_orphaned_post_wal(version);
    assert_eq!(gate.completion().watermark(), version + 1);
    assert_eq!(gate.last_committed(), version + 1);
    assert!(gate.is_orphaned_post_wal(version));
    gate.complete_orphaned_post_wal(version);
    assert_eq!(gate.durable_watermark(), 0);
    gate.mark_durable(version);
    gate.forget_orphaned_post_wal(version);
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(gate.durable_watermark(), version);
}

#[test]
fn post_wal_commit_does_not_register_an_orphan() {
    let gate = RepoTxGate::fresh();
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    guard.wal_accepted();
    guard.commit();
    assert!(!gate.is_active_post_wal(version));
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(gate.orphan_max(), 0);
    assert_eq!(gate.last_committed(), version);
    assert_eq!(gate.durable_watermark(), 0);
    assert_eq!(gate.completion().watermark(), version);
    assert!(gate.orphan_entry(version).is_none());
    assert!(gate.unresolved_wal_versions().is_empty());
    assert!(!gate.requires_reopen());
}
