use crate::repo::repo_types::BoxRepo;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::{Repo, Store};
use std::sync::Arc;

pub(crate) async fn copy_table_store(repo: &BoxRepo, from: &str, to: &str) -> DbResult<()> {
    let source = repo.store_get(from).await?;
    let destination = repo.store_get(to).await?;
    copy_store_contents(source, destination).await
}

pub(super) async fn copy_store_contents(
    source: Arc<dyn Store>,
    destination: Arc<dyn Store>,
) -> DbResult<()> {
    let mut stream = source.iter_stream(256);
    while let Some(page) = stream.next().await {
        let page = page?;
        destination.set_many(page).await?;
    }
    destination.flush().await
}
