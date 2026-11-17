//! Imported identity behavior: the minted disabled user resolves through
//! the ownership/lookup surfaces (`user_id`, `state_by_user_id`, `list_all`)
//! while the authentication surface (`lookup_by_name`) fails exactly like an
//! unknown user.

use shamir_connect::server::admin::UserDirectory;
use shamir_server_security::user_directory::FjallUserDirectory;
use tokio_util::sync::CancellationToken;

use crate::logical_import::common::build_fixture;
use crate::logical_import::common::export_args;
use crate::logical_import::common::export_now;
use crate::logical_import::common::import_args;
use crate::logical_import::common::import_now;
use crate::logical_import::common::runtime;

/// After import the source owner resolves by name through the directory
/// id surfaces and stays visible to ownership lookups (list_all /
/// state_by_user_id) without any credential material, while the auth
/// snapshot `lookup_by_name` returns EXACTLY the unknown-user result.
#[test]
fn imported_identity_is_visible_to_ownership_and_invisible_to_auth() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let target = root.join("target");
    import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap();

    let directory = FjallUserDirectory::open(target.join("users")).unwrap();
    // Forward id resolution works.
    let user_id = directory.user_id("owner").expect("imported owner resolves");
    // Reverse (ownership) resolution works and carries no privileges.
    let state = directory
        .state_by_user_id(&user_id)
        .expect("state_by_user_id resolves the imported identity");
    assert_eq!(state.username, "owner");
    assert!(state.database.is_none());
    assert!(state.roles.is_empty());
    assert!(!state.superuser);
    assert!(!state.replicator);
    // Ownership lookups see the disabled identity.
    assert!(directory
        .list_all()
        .unwrap()
        .iter()
        .any(|(_, state)| state.username == "owner"));
    // Auth lookup fails identically for the imported identity and an
    // unknown name — the equality is the contract, not just failure.
    assert_eq!(
        directory.lookup_by_name("owner").map(|record| record.salt),
        directory
            .lookup_by_name("never-existed")
            .map(|record| record.salt)
    );
    assert!(directory.lookup_by_name("owner").is_none());
}
