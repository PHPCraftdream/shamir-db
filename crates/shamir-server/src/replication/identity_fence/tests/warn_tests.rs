//! Adoption over a pre-existing durable bookmark is observable exactly once,
//! without timing or global subscribers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_db::engine::meta::repl::bookmark_v3::{save_bookmark_v3, BookmarkV3};
use tracing::instrument::WithSubscriber;
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

use super::identity_fence_tests::{build_memory, repo};
use crate::replication::identity_fence::checks;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;

struct AdoptionWarnings(Arc<AtomicUsize>);

impl<S: Subscriber> Layer<S> for AdoptionWarnings {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::WARN
            && event
                .metadata()
                .target()
                .ends_with("identity_fence::checks")
        {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[tokio::test]
async fn existing_bookmark_adopts_once_with_one_warning() {
    let warnings = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(AdoptionWarnings(warnings.clone()));
    async {
        let leader = build_memory().await;
        let follower = build_memory().await;
        // Seed a v3 follower bookmark (v3-only world): adoption must adopt
        // over it exactly once with one warning, preserving the cursor.
        let info = repo(&follower).tx_info_store().await.unwrap();
        save_bookmark_v3(
            &info,
            &BookmarkV3 {
                fence: None,
                bookmark: 7,
                clean_through: None,
                outstanding_receipt: None,
            },
        )
        .await
        .unwrap();
        let source = InProcessReplSource::new(leader);
        let hello = source.hello("probe").await.unwrap();
        checks::fence_hello_recorded(&follower, "app", "main", &hello, None)
            .await
            .unwrap();
        checks::fence_hello_recorded(&follower, "app", "main", &hello, None)
            .await
            .unwrap();
        assert_eq!(warnings.load(Ordering::Relaxed), 1);
        assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 7);
    }
    .with_subscriber(subscriber)
    .await;
}
