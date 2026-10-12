use shamir_db::engine::BoundedJournalRead;
use shamir_db::ShamirDb;
use shamir_tunables::instance_defaults::JOURNAL_BACKFILL_LIMIT;

use super::journal_source::JournalSource;

pub(super) struct DbJournalSource<'a> {
    pub db: &'a ShamirDb,
    pub name: &'a str,
}

impl JournalSource for DbJournalSource<'_> {
    async fn read(&self, repo: &str, from: u64) -> Result<BoundedJournalRead, String> {
        self.db
            .read_changelog_bounded(self.name, repo, from, JOURNAL_BACKFILL_LIMIT)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "journal repository unavailable".to_owned())
    }

    async fn progress(&self, repo: &str, target: u64) -> Result<(), String> {
        match self
            .db
            .await_changelog_progress(self.name, repo, target)
            .await
            .map_err(|error| error.to_string())?
        {
            Some(shamir_tx::repo_tx_gate::VisibleOutcome::Visible) => Ok(()),
            Some(shamir_tx::repo_tx_gate::VisibleOutcome::ReopenRequired) | None => {
                Err("journal repository unavailable or requires reopen".to_owned())
            }
        }
    }
}
