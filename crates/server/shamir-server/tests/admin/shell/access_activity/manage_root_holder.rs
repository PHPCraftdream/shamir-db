//! Recorded rights gap: a NON-bootstrap holder of `Manage(Root)` cannot be
//! created through the public wire today.
//!
//! No public grant op can express a `Manage(Root)` ACL. `ResourceRef`
//! (`crates/query/shamir-query-types/src/admin/access.rs:30`) has no `Root`
//! variant, so the typed `ddl::chmod` / `ddl::chown` / `ddl::chgrp`
//! builders (`.../resources/chmod.rs:8`, `chown.rs:8`, `chgrp.rs:8`) cannot
//! address `ResourcePath::Root`. The only public "grant" builder,
//! `ddl::grant_role`
//! (`crates/query/shamir-query-builder/src/ddl/policy/auth.rs:121`),
//! attaches an opaque role LABEL: roles carry no permission mapping, and the
//! literal `"superuser"` string is reserved at the directory write boundary
//! (`crates/query/shamir-query-types/src/wire/db_message.rs:109-115`).
//! `CreateScramUser.roles` is opaque for the same reason. The only
//! non-bootstrap principal with admin reach is therefore a full superuser
//! (`SetSuperuser`), which is not a `Manage(Root)` holder.
//!
//! This test is the honest witness of the gap: a wire-created SCRAM
//! principal with no superuser flag and no permission is refused with the
//! typed `permission_denied` by `user list` (which requires `Manage(Root)`),
//! while the bootstrap admin on the SAME server succeeds. Mutant: deleting
//! the `authorize_access(..., ResourcePath::Root, Action::Manage)` gate at
//! `crates/engine/shamir-db/src/shamir_db/execute/admin_meta/admin_list.rs:78`
//! lets `user list` succeed for the restricted principal, so the refusal
//! assertion turns red.

use super::harness::{
    overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture, SHELL_USER,
};
use super::secure_fixture::{boot_secure, run_with_secret};
use super::state_data_common::{data, refusal, scram_profile};

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

#[test]
fn no_non_bootstrap_manage_root_grant_is_reachable_today() {
    let secure = boot_secure("no_non_bootstrap_manage_root_grant_is_reachable_today");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    let user_password = seed_shell_user(fixture);

    // The restricted profile replaces the admin credentials: same server,
    // same catalogue, only the principal changes.
    let restricted = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string())
        .replace("username: admin\n", &format!("username: {SHELL_USER}\n"));
    overwrite_profile(fixture, &restricted);

    fixture.guard.mark("gap: user list without Manage(Root)");
    let run = run_with_secret(fixture, user_password.clone(), false, &["user", "list"]);
    assert!(
        !run.contains_canary(fixture.canary.as_str())
            && !run.contains_canary(user_password.as_str()),
        "secret leaked in the restricted run"
    );
    refusal(&run, "user", "permission_denied", 4);

    // Control: the SAME server still answers the bootstrap admin, so the
    // denial above is the server's authorization decision, not a dead
    // fixture or a local shell pre-judgement.
    use_admin_profile(fixture);
    let directory = data(&run_shell(fixture, &["user", "list"]), "user");
    assert!(
        directory["users"]
            .as_array()
            .is_some_and(|users| users.iter().any(|entry| entry["name"] == SHELL_USER)),
        "the bootstrap admin must still read the directory of the same server"
    );
}
