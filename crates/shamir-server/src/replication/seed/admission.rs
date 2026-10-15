use shamir_db::types::value::QueryValue;
use shamir_db::{SeedRefusal, ShamirDb};
use shamir_query_types::admin::ReplStream;

use crate::replication::ReplError;

pub(crate) async fn admit(
    home: &ShamirDb,
    name: &str,
    streams: &[ReplStream],
    db: &str,
    repo: &str,
) -> Result<(), ReplError> {
    super::terminal::validate_target(home, streams, db, repo).await?;
    if super::fence::load_receipt(home, db, repo).await?.is_none() {
        return Ok(());
    }
    let table = home
        .system_store()
        .subscriptions_table()
        .await
        .map_err(|error| ReplError::Start(error.to_string()))?;
    let refs = shamir_collections::TMap::default();
    // S5c-X F1 (Round 6): the FilterContext is built engine-side; no
    // `&Interner` crosses the boundary.
    let ctx = table
        .info_store_read_view()
        .filter_context(&refs)
        .await
        .map_err(|error| ReplError::Start(error.to_string()))?;
    let result = table
        .read(
            &shamir_db::query::read::ReadQuery::new("subscriptions"),
            &ctx,
        )
        .await
        .map_err(|error| ReplError::Start(error.to_string()))?;
    let admitted = result.records.iter().any(|row| {
        let row = row.as_value();
        row["name"].as_str() == Some(name)
            && row.get("seed_resume_validated") == Some(&QueryValue::Bool(true))
    });
    if !admitted {
        return Err(SeedRefusal::ConflictingBinding.into());
    }
    Ok(())
}
