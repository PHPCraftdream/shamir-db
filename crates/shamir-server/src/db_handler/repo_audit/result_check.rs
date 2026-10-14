use shamir_types::types::value::QueryValue;

pub(super) fn is_successful_alter_result(
    result: &shamir_query_types::read::QueryResult,
    target: &str,
) -> bool {
    let Some(QueryValue::Map(fields)) = result.value.as_ref() else {
        return false;
    };
    matches!(fields.get("altered"), Some(QueryValue::Bool(true)))
        && matches!(fields.get("altered_repo"), Some(QueryValue::Str(repo)) if repo == target)
}
