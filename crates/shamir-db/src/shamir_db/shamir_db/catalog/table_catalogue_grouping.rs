use shamir_collections::TFxMap;
use shamir_types::types::value::QueryValue;

/// Groups table catalogue records by `(database, repository)` in one pass;
/// `visit` is called once per record (the cost seam the boot oracle counts).
pub(crate) fn group_tables_by_repo<F>(
    table_records: &[QueryValue],
    mut visit: F,
) -> TFxMap<(&str, &str), Vec<&QueryValue>>
where
    F: FnMut(),
{
    let mut tables_by_repo: TFxMap<(&str, &str), Vec<&QueryValue>> = TFxMap::default();
    for table in table_records {
        visit();
        if let (Some(db), Some(repo)) = (table["db_name"].as_str(), table["repo_name"].as_str()) {
            tables_by_repo.entry((db, repo)).or_default().push(table);
        }
    }
    tables_by_repo
}
