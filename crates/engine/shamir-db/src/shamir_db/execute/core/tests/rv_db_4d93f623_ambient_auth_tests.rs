use crate::access::{Actor, ResourceMeta, ResourcePath};
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::ShamirDb;
use shamir_query_builder::batch::Batch;

#[tokio::test]
async fn rv_db_4d93f623_ambient_access_denied_before_empty_batch() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("ambient_test").await.unwrap();
    shamir
        .add_repo(
            "ambient_test",
            RepoConfig::new("secret", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &ResourcePath::database("ambient_test"),
            &ResourceMeta::open(),
        )
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &ResourcePath::store("ambient_test", "secret"),
            &ResourceMeta {
                owner: Actor::User(1),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();

    let mut batch = Batch::new();
    batch.id("empty");
    let mut request = batch
        .to_request_via_msgpack()
        .expect("batch request encoding");
    request.interner_epochs.insert("secret".into(), 0);

    // Positive control: database and store both permit this actor.
    shamir
        .set_resource_meta(
            &ResourcePath::store("ambient_test", "secret"),
            &ResourceMeta::open(),
        )
        .await
        .unwrap();
    assert!(shamir
        .execute_as(Actor::User(2), "ambient_test", &request)
        .await
        .is_ok());

    // DB remains readable, but Store access is denied.
    shamir
        .set_resource_meta(
            &ResourcePath::store("ambient_test", "secret"),
            &ResourceMeta {
                owner: Actor::User(1),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();
    let response = shamir
        .execute_as(Actor::User(2), "ambient_test", &request)
        .await
        .expect("denied ambient repositories are skipped");
    assert!(!response.interner_delta.contains_key("secret"));
}
