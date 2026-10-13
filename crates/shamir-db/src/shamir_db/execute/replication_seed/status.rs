use shamir_engine::meta::repo_identity::RepoIdentity;

use crate::types::value::QueryValue;
use crate::{DbResult, ShamirDb};

use super::catalogue::streams;

impl ShamirDb {
    pub(crate) async fn seed_status_entries(
        &self,
        records: Vec<QueryValue>,
    ) -> DbResult<Vec<QueryValue>> {
        let mut entries = Vec::new();
        for record in records {
            let profile = record["profile"].as_str().unwrap_or("");
            let targets = streams(self, profile).await.unwrap_or_default();
            let mut emitted = false;
            for stream in targets {
                let Some(name) = stream.scope.repo else {
                    continue;
                };
                let Some(repo) = self
                    .get_db(&stream.scope.db)
                    .and_then(|db| db.get_repo(&name))
                else {
                    continue;
                };
                let mut fields = crate::types::common::new_map();
                fields.insert("db".into(), QueryValue::Str(stream.scope.db));
                fields.insert("repo".into(), QueryValue::Str(name));
                fields.insert("name".into(), record["name"].clone());
                fields.insert("state".into(), record["state"].clone());
                fields.insert("subscription_state".into(), record["state"].clone());
                fields.insert("status_message".into(), record["status_message"].clone());
                fields.insert(
                    "last_terminal_reason".into(),
                    record["last_terminal_reason"].clone(),
                );
                let identity = shamir_engine::meta::repl_bookmark::load_fence_record(
                    &repo.tx_info_store().await?,
                )
                .await?;
                fields.insert(
                    "recorded_identity".into(),
                    super::super::helpers::to_qv(&identity.map(|record| RepoIdentity {
                        lineage_id: record.lineage_id,
                        history_epoch: record.history_epoch,
                    })),
                );
                fields.insert(
                    "observed_identity".into(),
                    super::super::helpers::to_qv(
                        &identity.and_then(|record| record.observed_identity),
                    ),
                );
                let bookmark = repo.replication_bookmark().await?;
                fields.insert(
                    "upstream_bookmark".into(),
                    super::super::helpers::to_qv(&bookmark),
                );
                fields.insert(
                    "next_pull_version".into(),
                    super::super::helpers::to_qv(&bookmark.checked_add(1)),
                );
                fields.insert(
                    "local_consistent_version".into(),
                    super::super::helpers::to_qv(&repo.consistent_version().await?),
                );
                let gap = repo.certified_gap_interval().await?;
                fields.insert(
                    "certified_gap_first".into(),
                    super::super::helpers::to_qv(&gap.map(|(first, _)| first)),
                );
                fields.insert(
                    "certified_gap_through".into(),
                    super::super::helpers::to_qv(&gap.map(|(_, through)| through)),
                );
                for key in [
                    "artifact_digest",
                    "source_db",
                    "source_repo",
                    "source_lineage_id",
                    "source_history_epoch",
                    "seed_source_version",
                    "local_baseline_version",
                    "schema_digest",
                ] {
                    fields.insert(key.into(), QueryValue::Null);
                }
                let install =
                    shamir_engine::repo::SeedInstallState::load(&repo.tx_info_store().await?)
                        .await?;
                let (state, receipt) = match install {
                    Some(shamir_engine::repo::SeedInstallState::Complete(receipt)) => {
                        ("complete", Some(receipt))
                    }
                    Some(shamir_engine::repo::SeedInstallState::Installing(receipt)) => {
                        ("installing", Some(receipt))
                    }
                    None => ("ordinary", None),
                };
                fields.insert("installation_state".into(), QueryValue::Str(state.into()));
                if let Some(receipt) = receipt {
                    fields.insert(
                        "artifact_digest".into(),
                        QueryValue::Str(receipt.artifact_digest),
                    );
                    fields.insert("source_db".into(), QueryValue::Str(receipt.source_db));
                    fields.insert("source_repo".into(), QueryValue::Str(receipt.source_repo));
                    fields.insert(
                        "source_lineage_id".into(),
                        super::super::helpers::to_qv(&receipt.source_identity.lineage_id),
                    );
                    fields.insert(
                        "source_history_epoch".into(),
                        super::super::helpers::to_qv(&receipt.source_identity.history_epoch),
                    );
                    fields.insert(
                        "seed_source_version".into(),
                        super::super::helpers::to_qv(&receipt.source_version),
                    );
                    fields.insert(
                        "local_baseline_version".into(),
                        super::super::helpers::to_qv(&receipt.local_baseline),
                    );
                    fields.insert(
                        "schema_digest".into(),
                        QueryValue::Str(receipt.schema_digest),
                    );
                }
                entries.push(QueryValue::Map(fields));
                emitted = true;
            }
            if !emitted {
                entries.push(record);
            }
        }
        Ok(entries)
    }
}
