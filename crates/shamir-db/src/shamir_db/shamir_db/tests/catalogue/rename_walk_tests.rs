//! Parent UPDATE/DELETE issued through the PUBLIC `ShamirDb` API must still
//! succeed while the FK CHILD table's rename is in doubt: the write path's
//! read-only walks (reverse-FK cache rebuild) resolve the fenced child name
//! through `DbTableResolver::resolve_unfenced`, so the fence must not break
//! ordinary writes on the parent.

use crate::shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_builder::write::UpdateReturnMode;
use shamir_types::mpack;
use shamir_types::types::record_id::RecordId;

#[tokio::test]
async fn parent_update_and_delete_succeed_while_child_rename_in_doubt() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let db = ShamirDb::init_memory().await.unwrap();
        db.create_db("fk-walk").await.unwrap();

        // Real repo with a real FK: employees.dept_id -> departments.dept_id
        // (NoAction, so the parent ops themselves are allowed to proceed).
        let mut b = Batch::new();
        b.id(1);
        b.create_repo(
            "cr",
            ddl::create_repo("main").tables(["departments", "employees"]),
        );
        db.execute(
            "fk-walk",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();

        let fk_id = RecordId::system("fk_walk_child_schema");
        let rules = vec![shamir_engine::validator::schema::FieldRule {
            path: vec!["dept_id".to_string()],
            ty: shamir_engine::validator::schema::type_tag::TypeTag::Int,
            constraints: shamir_engine::validator::schema::constraints::Constraints {
                foreign_key: Some(
                    shamir_engine::validator::schema::ForeignKeyRef::with_on_delete(
                        "departments",
                        "dept_id",
                        shamir_query_types::admin::FkAction::NoAction,
                    ),
                ),
                required: true,
                ..Default::default()
            },
            keyset_safe: false,
        }];
        db.compile_table_schema("fk-walk", "main", "employees", fk_id, rules)
            .await
            .expect("child FK schema activation must succeed");

        // Seed one parent row + one referencing child row via the public API.
        let mut b = Batch::new();
        b.id(2);
        b.insert(
            "ins_dept",
            write::insert("departments").row(mpack!({ "dept_id": 100, "name": "Engineering" })),
        );
        db.execute(
            "fk-walk",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();

        let mut b = Batch::new();
        b.id(3);
        b.insert(
            "ins_emp",
            write::insert("employees").row(mpack!({ "name": "Alice", "dept_id": 100 })),
        );
        db.execute(
            "fk-walk",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();

        // Force the CHILD rename into the in-doubt (fenced) state: the direct
        // engine-level `rename_table_stores_with` + `InDoubt` outcome (same
        // pattern as the engine walk tests; `rename_table_as` itself refuses
        // schema-bearing tables, and the FK child carries a schema).
        let repo = db.get_db("fk-walk").unwrap().get_repo("main").unwrap();
        assert!(matches!(
            repo.rename_table_stores_with("employees", "employees2", |_| async {
                crate::engine::repo::RenamePersistOutcome::InDoubt(crate::DbError::Storage(
                    "uncertain".into(),
                ))
            })
            .await,
            Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
        ));
        assert!(repo.has_rename_in_doubt());
        assert!(matches!(
            repo.get_table("employees").await,
            Err(crate::DbError::Conflict(message)) if message.starts_with("rename in doubt:")
        ));

        // The setup inserts warmed the reverse-FK cache; drop it so the parent
        // operations below must rebuild it through `resolve_unfenced`.
        repo.fk_reverse_cache().invalidate();

        // Parent UPDATE via the public execute path: its reverse-FK walk must
        // resolve the fenced child through `resolve_unfenced` and succeed.
        let mut b = Batch::new();
        b.id(4);
        b.try_update(
            "upd_dept",
            write::update("departments")
                .where_(filter::eq("dept_id", 100))
                .set(doc().set("name", "Platform").unwrap())
                .returning(UpdateReturnMode::All),
        )
        .unwrap();
        let resp = db
            .execute(
                "fk-walk",
                &b.to_request_via_msgpack().expect("batch request encoding"),
            )
            .await
            .expect("parent update must succeed while the child is fenced");
        assert_eq!(resp.results["upd_dept"].records.len(), 1);

        // Parent DELETE likewise: the read-only FK discovery walk must not
        // trip over the fence.
        repo.fk_reverse_cache().invalidate();
        let mut b = Batch::new();
        b.id(5);
        b.try_delete(
            "del_dept",
            write::delete("departments")
                .where_(filter::eq("dept_id", 100))
                .returning(),
        )
        .unwrap();
        let resp = db
            .execute(
                "fk-walk",
                &b.to_request_via_msgpack().expect("batch request encoding"),
            )
            .await
            .expect("parent delete must succeed while the child is fenced");
        assert_eq!(resp.results["del_dept"].records.len(), 1);
    })
    .await
    .expect("fk walk test exceeded 30 seconds");
}
