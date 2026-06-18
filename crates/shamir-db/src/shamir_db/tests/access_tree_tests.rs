//! Tests for `ShamirDb::access_tree` — the access-control tree assembly
//! that backs the `access_tree` DDL op and the `access-tree` CLI command.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::ShamirDb;
use shamir_types::access::{principal_id, Actor, ResourceMeta, ResourcePath};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

/// Find a child node by its `name`, panicking if absent. Children order
/// is non-deterministic (dashmap iteration), so always look up by name.
fn child<'a>(node: &'a QueryValue, name: &str) -> &'a QueryValue {
    node["children"]
        .as_array()
        .expect("children array")
        .iter()
        .find(|c| c["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("child '{name}' not found"))
}

#[tokio::test]
async fn access_tree_structure_meta_and_principals() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    // A real group so the table's group resolves to a name.
    let gid = shamir.create_group("devs").await.unwrap();

    // A user record so the table's owner resolves to a name, and the
    // group membership renders the member name.
    let alice = principal_id("alice");
    shamir.add_group_member(gid, alice).await.unwrap();
    {
        let table = shamir.system_store().users_table().await.unwrap();
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("users"),
            key: mpack!({ "name": "alice" }),
            value: mpack!({ "name": "alice" }),
        };
        shamir
            .system_store()
            .set_via_implicit_tx(&table, &op)
            .await
            .unwrap();
    }

    // chown alice + chgrp devs + chmod 0o750 on the table.
    let meta = ResourceMeta {
        owner: Actor::User(alice),
        group: Some(gid),
        mode: 0o750,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &meta)
        .await
        .unwrap();

    let tree = shamir.access_tree(None, None).await.unwrap();

    // ── resource hierarchy: root → db → store → table ──
    let root = &tree["resources"];
    assert_eq!(root["kind"].as_str(), Some("root"));
    let db = child(root, "testdb");
    assert_eq!(db["kind"].as_str(), Some("database"));
    let store = child(db, "data");
    assert_eq!(store["kind"].as_str(), Some("store"));
    let table = child(store, "users");
    assert_eq!(table["kind"].as_str(), Some("table"));
    assert_eq!(table["mode"].as_u64(), Some(0o750));
    assert_eq!(table["owner"].as_u64(), Some(alice));
    assert_eq!(table["owner_name"].as_str(), Some("alice"));
    assert_eq!(table["group"].as_u64(), Some(gid));
    assert_eq!(table["group_name"].as_str(), Some("devs"));

    // ── principals ──
    let users = tree["principals"]["users"].as_array().unwrap();
    assert!(
        users.iter().any(|u| u["name"].as_str() == Some("alice")),
        "alice should appear in principals.users"
    );
    let groups = tree["principals"]["groups"].as_array().unwrap();
    let devs = groups
        .iter()
        .find(|g| g["name"].as_str() == Some("devs"))
        .expect("devs group present");
    assert_eq!(devs["id"].as_u64(), Some(gid));
    let members = devs["members"].as_array().unwrap();
    assert!(
        members
            .iter()
            .any(|m| m["id"].as_u64() == Some(alice) && m["name"].as_str() == Some("alice")),
        "alice should be a resolved member of devs"
    );
}

#[tokio::test]
async fn access_tree_depth_caps_the_hierarchy() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("t"));
    shamir.add_repo("testdb", config).await.unwrap();

    // depth 0 → root only, no children at all.
    let t0 = shamir.access_tree(Some(0), None).await.unwrap();
    assert!(t0["resources"]["children"].as_array().unwrap().is_empty());

    // depth 1 → databases present, but no stores beneath them.
    let t1 = shamir.access_tree(Some(1), None).await.unwrap();
    let db = child(&t1["resources"], "testdb");
    assert!(db["children"].as_array().unwrap().is_empty());

    // depth 2 → stores present, but no tables beneath them.
    let t2 = shamir.access_tree(Some(2), None).await.unwrap();
    let store = child(child(&t2["resources"], "testdb"), "data");
    assert!(store["children"].as_array().unwrap().is_empty());

    // full → tables present.
    let tf = shamir.access_tree(None, None).await.unwrap();
    let table = child(child(child(&tf["resources"], "testdb"), "data"), "t");
    assert_eq!(table["kind"].as_str(), Some("table"));
}

#[tokio::test]
async fn access_tree_dispatch_admin_gate_denies_non_admin() {
    use crate::query::batch::{BatchError, BatchRequest, BatchResponse};

    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("t"));
    shamir.add_repo("testdb", config).await.unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.access_tree("tree", ddl::access_tree());
    let req: BatchRequest = b.to_request_via_msgpack();

    // The op carries the tree only when the caller is allowed.
    let tree_present = |res: &Result<BatchResponse, BatchError>| -> bool {
        matches!(res, Ok(r) if r
            .results
            .get("tree")
            .and_then(|q| q.records.first())
            .and_then(|rec| rec.get_value("access_tree"))
            .and_then(|t| t.get("resources"))
            .is_some())
    };

    // System (admin) → allowed.
    let sys = shamir.execute_as(Actor::System, "testdb", &req).await;
    assert!(tree_present(&sys), "System must receive the access tree");

    // Regular authenticated user → denied (no tree surfaces).
    let bob = principal_id("bob");
    let user = shamir.execute_as(Actor::User(bob), "testdb", &req).await;
    assert!(
        !tree_present(&user),
        "a non-admin user must be denied the access tree"
    );
}

#[tokio::test]
async fn access_tree_db_filter_scopes_to_one_database() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("alpha").await;
    shamir.create_db("beta").await;
    let cfg_a =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("t"));
    shamir.add_repo("alpha", cfg_a).await.unwrap();

    let tree = shamir.access_tree(None, Some("alpha")).await.unwrap();
    let dbs = tree["resources"]["children"].as_array().unwrap();
    assert_eq!(dbs.len(), 1, "filter must yield exactly one database");
    assert_eq!(dbs[0]["name"].as_str(), Some("alpha"));
}
