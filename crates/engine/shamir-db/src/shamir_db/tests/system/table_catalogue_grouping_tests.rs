use shamir_types::types::value::QueryValue;

use crate::shamir_db::shamir_db::group_tables_by_repo;

fn table(db: &str, repo: &str, name: &str) -> QueryValue {
    let mut fields = shamir_types::types::common::new_map();
    fields.insert("db_name".into(), QueryValue::Str(db.into()));
    fields.insert("repo_name".into(), QueryValue::Str(repo.into()));
    fields.insert("table_name".into(), QueryValue::Str(name.into()));
    QueryValue::Map(fields)
}

#[test]
fn grouping_partitions_catalogue_records_and_counts_malformed_rows() {
    let records = vec![
        table("alpha", "shared", "alpha_table"),
        table("beta", "shared", "beta_table"),
        table("alpha", "other", "other_table"),
        {
            let mut malformed = shamir_types::types::common::new_map();
            malformed.insert("db_name".into(), QueryValue::Str("invalid".into()));
            QueryValue::Map(malformed)
        },
        QueryValue::Null,
    ];
    let mut visits = 0;
    let grouped = group_tables_by_repo(&records, || visits += 1);

    assert_eq!(grouped.len(), 3);
    assert_eq!(grouped.get(&("alpha", "shared")).unwrap()[0], &records[0]);
    assert_eq!(grouped.get(&("beta", "shared")).unwrap()[0], &records[1]);
    assert_eq!(grouped.get(&("alpha", "other")).unwrap()[0], &records[2]);
    assert_eq!(visits, records.len());
    assert_eq!(grouped.values().map(Vec::len).sum::<usize>(), 3);
}

#[test]
fn large_catalogue_is_visited_once_not_once_per_repository() {
    let repositories = 24;
    let tables_per_repository = 16;
    let records: Vec<_> = (0..repositories)
        .flat_map(|repo| {
            (0..tables_per_repository).map(move |table_index| {
                table(
                    "database",
                    &format!("repo-{repo}"),
                    &format!("table-{table_index}"),
                )
            })
        })
        .collect();
    let mut visits = 0;
    let grouped = group_tables_by_repo(&records, || visits += 1);

    assert_eq!(grouped.len(), repositories);
    assert_eq!(visits, records.len());
    assert!(grouped
        .values()
        .all(|tables| tables.len() == tables_per_repository));
}
