use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::TableConfig;
use crate::validator::registry::validator_db::{
    reset_staged_rechecks, staged_rechecks, ValidatorDb,
};
use shamir_query_builder::write::{doc, insert};
use shamir_tx::IsolationLevel;
use shamir_types::access::Actor;

#[tokio::test]
async fn rv_engine_0db9257b_staged_probe_is_not_linear() {
    let repo_config = RepoConfig {
        name: "staged_probe_counter".to_owned(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("items")],
    };
    let repo = RepoInstance::from_factory(
        repo_config.name.clone(),
        repo_config.factory,
        repo_config.tables,
    )
    .await
    .unwrap();
    let table = repo.get_table("items").await.unwrap();
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tx.set_implicit(true);

    reset_staged_rechecks();
    for i in 0..200i64 {
        let db = ValidatorDb::new(&tx, &table, None);
        assert!(!db
            .exists_in_self("uid", &shamir_types::types::value::QueryValue::Int(i), None)
            .await
            .unwrap());
        let op = insert("items").row(doc().set("uid", i).unwrap()).build();
        table
            .execute_insert_tx(&op, &mut tx, false, None, &Actor::System)
            .await
            .unwrap();
    }
    {
        let db = ValidatorDb::new(&tx, &table, None);
        assert!(db
            .exists_in_self(
                "uid",
                &shamir_types::types::value::QueryValue::Int(137),
                None,
            )
            .await
            .unwrap());
    }
    let rechecks = staged_rechecks();
    assert!(
        rechecks > 0,
        "the staged probe did not perform an exact recheck"
    );
    assert!(rechecks <= 400, "staged exact rechecks: {rechecks}");
    repo.commit_tx(tx).await.unwrap();
}
