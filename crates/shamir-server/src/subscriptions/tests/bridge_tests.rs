use std::future::Future;
use std::sync::Arc;

use shamir_db::engine::BoundedJournalRead;
use shamir_tx::ChangelogEvent;
use tokio::sync::broadcast;

use super::super::journal_drain::JournalDrain;
use super::super::journal_source::JournalSource;

struct ExhaustedSource;

impl JournalSource for ExhaustedSource {
    fn read(
        &self,
        _repo: &str,
        _from: u64,
    ) -> impl Future<Output = Result<BoundedJournalRead, String>> + Send {
        async { panic!("exhausted u64 cursor must not wrap into a history read") }
    }

    fn progress(
        &self,
        _repo: &str,
        _target: u64,
    ) -> impl Future<Output = Result<(), String>> + Send {
        async { panic!("exhausted cursor has no pending progress") }
    }
}

// Replaces integer-only watermark tests: exercises the actual drain boundary.
#[tokio::test]
async fn maximum_cursor_never_wraps_to_zero() {
    let (_tx, rx) = broadcast::channel::<Arc<ChangelogEvent>>(1);
    let repos = vec!["main".to_owned()];
    let mut states = vec![JournalDrain::new(u64::MAX)];
    JournalDrain::run(
        &ExhaustedSource,
        &repos,
        &mut [rx],
        &mut states,
        true,
        |_, _| async { panic!("exhausted cursor cannot deliver") },
        |_| async { panic!("exhausted cursor cannot report a gap") },
    )
    .await
    .unwrap();
    assert_eq!(states[0].cursor, u64::MAX);
}
