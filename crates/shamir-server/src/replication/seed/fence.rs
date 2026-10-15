use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::engine::repo::{SeedInstallState, SeedReceipt};
use shamir_db::{ResumeProbe, SeedRefusal, ShamirDb};
use shamir_query_types::admin::CreatePublicationOp;
use shamir_query_types::wire::repl::ReplResponse;

use super::super::super::error::ReplError;
use super::super::super::supervisor::{ReplSourceFactory, Subscription};
use crate::replication_seed::schema_as_names::schema_as_names;
use crate::replication_seed::schema_digest::schema_digest;
use crate::replication_seed::TableRecord;

pub(crate) struct ServerResumeProbe(pub ReplSourceFactory);

#[async_trait]
impl ResumeProbe for ServerResumeProbe {
    async fn hello(&self, upstream: &str) -> Result<ReplResponse, SeedRefusal> {
        let sub = Subscription {
            name: String::new(),
            upstream: upstream.into(),
            publication: String::new(),
            profile: String::new(),
            state: "paused".into(),
        };
        (self.0)(&sub)
            .hello("seed-resume-validation")
            .await
            .map_err(|_| SeedRefusal::UpstreamUnavailable)
    }
}

pub(crate) async fn load_receipt(
    home: &ShamirDb,
    db: &str,
    repo: &str,
) -> Result<Option<SeedReceipt>, ReplError> {
    let Some(repo) = home.get_db(db).and_then(|db| db.get_repo(repo)) else {
        return Ok(None);
    };
    let info = repo
        .tx_info_store()
        .await
        .map_err(|error| ReplError::Bookmark(error.to_string()))?;
    match SeedInstallState::load(&info)
        .await
        .map_err(|error| ReplError::Bookmark(error.to_string()))?
    {
        Some(SeedInstallState::Complete(receipt)) => {
            SeedInstallState::require_complete(&info, true)
                .await
                .map_err(|_| ReplError::Seed(SeedRefusal::IncompleteInstall))?;
            Ok(Some(receipt))
        }
        Some(SeedInstallState::Installing(_)) => {
            Err(ReplError::Seed(SeedRefusal::IncompleteInstall))
        }
        None => Ok(None),
    }
}

pub(crate) async fn live_schema(
    home: &ShamirDb,
    db: &str,
    repo: &str,
) -> Result<String, ReplError> {
    let instance = home
        .get_db(db)
        .and_then(|db| db.get_repo(repo))
        .ok_or_else(|| ReplError::UnknownFollowerRepo {
            db: db.into(),
            repo: repo.into(),
        })?;
    let manager = instance
        .repo_interner()
        .await
        .map_err(|error| ReplError::Transport(error.to_string()))?;
    let interner = manager
        .get()
        .await
        .map_err(|error| ReplError::Transport(error.to_string()))?;
    let mut names = instance.list_table_names();
    names.sort();
    let mut tables = Vec::new();
    for name in names {
        let row = home
            .system_store()
            .load_table_record(db, repo, &name)
            .await
            .map_err(|error| ReplError::Transport(error.to_string()))?
            .unwrap_or(shamir_db::types::value::QueryValue::Null);
        let schema = schema_as_names(
            row.get("schema")
                .unwrap_or(&shamir_db::types::value::QueryValue::Null),
            interner,
        )
        .map_err(|_| ReplError::Seed(SeedRefusal::WrongSchema))?;
        tables.push(TableRecord {
            name,
            schema,
            schema_version: u64::try_from(
                row.get("schema_version")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
            )
            .map_err(|_| ReplError::Seed(SeedRefusal::WrongSchema))?,
            source_enable_indexes: false,
            follower_enable_indexes: false,
            excluded_indexes: Vec::new(),
            live_records: 0,
        });
    }
    schema_digest(&tables).map_err(|_| ReplError::Seed(SeedRefusal::WrongSchema))
}

pub(crate) async fn publications(home: &ShamirDb) -> Result<Vec<CreatePublicationOp>, ReplError> {
    let table = home
        .system_store()
        .publications_table()
        .await
        .map_err(|e| ReplError::Transport(e.to_string()))?;
    let refs = shamir_collections::TMap::default();
    // S5c-X F1 (Round 6): the FilterContext is built engine-side; no
    // `&Interner` crosses the boundary.
    let ctx = table
        .info_store_read_view()
        .filter_context(&refs)
        .await
        .map_err(|e| ReplError::Transport(e.to_string()))?;
    let result = table
        .read(
            &shamir_db::query::read::ReadQuery::new("publications"),
            &ctx,
        )
        .await
        .map_err(|e| ReplError::Transport(e.to_string()))?;
    let mut publications = Vec::new();
    for row in result.records {
        let row = row.as_value();
        let name = row["name"]
            .as_str()
            .ok_or(ReplError::Seed(SeedRefusal::WrongPublication))?;
        let bytes = rmp_serde::to_vec_named(&row["scopes"])
            .map_err(|e| ReplError::Transport(e.to_string()))?;
        publications.push(CreatePublicationOp {
            create_publication: name.into(),
            scopes: rmp_serde::from_slice(&bytes)
                .map_err(|e| ReplError::Transport(e.to_string()))?,
        });
    }
    Ok(publications)
}

pub(crate) fn fence_hello(
    receipt: Option<&SeedReceipt>,
    response: &ReplResponse,
) -> Result<u64, ReplError> {
    if let Some(receipt) = receipt {
        let ReplResponse::Hello { repos, .. } = response else {
            return Err(SeedRefusal::MissingIdentity.into());
        };
        let advertised = repos
            .iter()
            .find(|repo| repo.db == receipt.source_db && repo.repo == receipt.source_repo)
            .ok_or(SeedRefusal::MissingIdentity)?;
        shamir_db::fence_metadata(
            receipt,
            advertised.repo_lineage_id,
            advertised.history_epoch,
            advertised.schema_digest.as_deref(),
        )?;
    }
    Ok(super::super::leader_epoch_of(response))
}

pub(crate) fn fence_pull(
    receipt: Option<&SeedReceipt>,
    response: &ReplResponse,
) -> Result<(), ReplError> {
    if let (
        Some(receipt),
        ReplResponse::Pull {
            repo_lineage_id,
            history_epoch,
            schema_digest,
            ..
        },
    ) = (receipt, response)
    {
        shamir_db::fence_metadata(
            receipt,
            *repo_lineage_id,
            *history_epoch,
            schema_digest.as_deref(),
        )?;
    }
    Ok(())
}

pub(crate) fn install_probe(home: &ShamirDb, factory: &ReplSourceFactory) {
    home.set_replication_resume_probe(Arc::new(ServerResumeProbe(Arc::clone(factory))));
}
