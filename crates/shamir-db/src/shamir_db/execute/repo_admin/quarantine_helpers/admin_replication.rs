use crate::shamir_db::execute::quarantine_cache::{
    global_sentinel_key, unresolvable_profile_reason,
};
use crate::shamir_db::execute::replication_seed::catalogue::streams;
use crate::types::value::QueryValue;
use crate::ShamirDb;
use shamir_query_types::admin::ReplDirection;

pub(crate) fn row_is_quarantined(row: &QueryValue) -> bool {
    row.get("state").and_then(|value| value.as_str()) == Some("resync_required")
        || row.get("quarantined") == Some(&QueryValue::Bool(true))
}

pub(crate) async fn quarantine_row_entries(
    shamir: &ShamirDb,
    profile: Option<&str>,
    reason: &str,
) -> (Vec<((String, String), String)>, Option<String>) {
    let Some(profile) = profile else {
        return (
            vec![(
                global_sentinel_key(),
                unresolvable_profile_reason("<missing>"),
            )],
            Some("quarantined subscription has no profile".into()),
        );
    };
    match streams(shamir, profile).await {
        Ok(streams) => {
            let mut entries = Vec::new();
            for stream in streams {
                if stream.direction == ReplDirection::Pull {
                    let repo = stream.scope.repo.unwrap_or_else(|| "*".to_string());
                    entries.push(((stream.scope.db, repo), reason.to_string()));
                }
            }
            (entries, None)
        }
        Err(error) => (
            vec![(global_sentinel_key(), unresolvable_profile_reason(profile))],
            Some(format!("profile resolution failed ({profile}): {error}")),
        ),
    }
}
