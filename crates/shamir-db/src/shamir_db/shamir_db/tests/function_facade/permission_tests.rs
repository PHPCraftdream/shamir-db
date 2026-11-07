use super::support::{echo_variant_wasm, echo_wasm, tamper_row};
use crate::access::{Actor, ResourceMeta, ResourcePath};
use crate::shamir_db::{FunctionState, ShamirDb};
use crate::DbError;

const OWNER: Actor = Actor::User(1);
const OTHER: Actor = Actor::User(2);

async fn db_with_private_function() -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("init");
    db.create_function_from_wasm_as("f", &echo_wasm(), false, OWNER)
        .await
        .expect("create as owner");
    db
}

fn path_of(name: &str) -> ResourcePath {
    ResourcePath::Function {
        name: name.to_string(),
    }
}

#[tokio::test]
async fn describe_needs_read_on_the_function() {
    let db = db_with_private_function().await;

    let err = db
        .describe_function_as("f", OTHER)
        .await
        .expect_err("other user must be denied");

    assert!(err.to_string().contains("access denied"), "got: {err}");
    assert_eq!(
        db.describe_function_as("f", OWNER)
            .await
            .expect("owner")
            .name,
        "f"
    );
    assert_eq!(
        db.describe_function_as("f", Actor::System)
            .await
            .expect("system")
            .name,
        "f"
    );
}

#[tokio::test]
async fn export_needs_write_on_the_function() {
    let db = db_with_private_function().await;

    let err = db
        .export_function_as("f", OTHER)
        .await
        .expect_err("other user must be denied");

    assert!(err.to_string().contains("access denied"), "got: {err}");
    let export = db.export_function_as("f", OWNER).await.expect("owner");
    assert_eq!(export.bytes, echo_wasm());
}

#[tokio::test]
async fn read_only_other_may_describe_but_export_needs_write_bit() {
    let db = db_with_private_function().await;
    let meta = |mode| ResourceMeta {
        owner: OWNER,
        group: None,
        mode,
    };

    db.set_resource_meta(&path_of("f"), &meta(0o744))
        .await
        .expect("set 0o744");
    assert_eq!(
        db.describe_function_as("f", OTHER)
            .await
            .expect("read")
            .name,
        "f"
    );
    let err = db
        .export_function_as("f", OTHER)
        .await
        .expect_err("read-only other cannot export");
    assert!(err.to_string().contains("access denied"), "got: {err}");

    db.set_resource_meta(&path_of("f"), &meta(0o766))
        .await
        .expect("set 0o766");
    let export = db.export_function_as("f", OTHER).await.expect("write bit");
    assert_eq!(export.bytes, echo_wasm());
}

#[tokio::test]
async fn denial_is_access_denied_not_not_found() {
    let db = db_with_private_function().await;

    let missing = db
        .describe_function_as("missing", Actor::System)
        .await
        .expect_err("missing name");
    let denied = db
        .describe_function_as("f", OTHER)
        .await
        .expect_err("private function");

    assert!(matches!(missing, DbError::NotFound(_)), "got: {missing}");
    assert!(!matches!(denied, DbError::NotFound(_)), "got: {denied}");
    assert!(
        denied.to_string().contains("access denied"),
        "got: {denied}"
    );
}

#[tokio::test]
async fn replacing_a_corrupt_function_needs_write() {
    let db = db_with_private_function().await;
    tamper_row(&db, "f", &echo_variant_wasm()).await;
    assert!(db.export_function("f").await.is_err());
    assert!(matches!(
        db.describe_function("f").await.expect("describe").state,
        FunctionState::Corrupt { .. }
    ));

    let denied = db
        .create_function_from_wasm_as("f", &echo_wasm(), true, OTHER)
        .await;
    assert!(denied.is_err(), "non-writer must not replace");
    assert!(matches!(
        db.describe_function("f").await.expect("describe").state,
        FunctionState::Corrupt { .. }
    ));

    db.create_function_from_wasm_as("f", &echo_wasm(), true, OWNER)
        .await
        .expect("owner replaces");
    assert_eq!(
        db.describe_function("f").await.expect("describe").state,
        FunctionState::Loaded
    );
}
