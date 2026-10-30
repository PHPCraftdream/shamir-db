use qb::{filter, query::Query, select};

#[test]
fn renamed_dependency_expands_filter_and_repo_query() {
    let actual = qb::filter!(profile.age >= 18 && status == "active");
    let expected = filter::and([
        filter::gte(["profile", "age"], 18),
        filter::eq("status", "active"),
    ]);
    assert_eq!(actual, expected);

    let actual = qb::q!(from archive.users where profile.age >= 18 select profile.age as years);
    let expected = Query::with_repo("archive", "users")
        .where_(filter::gte(["profile", "age"], 18))
        .select([select::field_as(["profile", "age"], "years")])
        .build();
    assert_eq!(actual, expected);
}
