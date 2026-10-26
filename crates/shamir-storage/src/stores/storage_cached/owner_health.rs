//! Owner-health plumbing for the `CachedStore` FIFO owner task — one
//! closely-coupled group moved out of the line-capped `storage_cached.rs`
//! (RV-STORAGE-E02C8A80 round 2): the storage-error constructor, the
//! health-watch type, sticky failure publication, the owner RAII guard,
//! and the reply/health race helper. No behavior change.

use crate::error::{DbError, DbResult};
use tokio::sync::{oneshot, watch};

pub(crate) fn storage_err(msg: impl Into<String>) -> DbError {
    DbError::Storage(msg.into())
}

/// Health watch payload: `None` = healthy, `Some(msg)` = sticky failure.
pub(crate) type HealthWatch = watch::Sender<Option<String>>;

/// Publishes a terminal failure if none existed yet (idempotent).
pub(crate) fn publish_failure(health: &HealthWatch, msg: String) {
    if health.borrow().is_none() {
        health.send_replace(Some(msg));
    }
}

/// RAII: dropped when the owner future is dropped (normal exit, panic
/// unwind, or abort before the first poll — built before `tokio::spawn`).
/// Marks the store terminally failed on every exit path.
pub(crate) struct OwnerGuard {
    pub(crate) health: HealthWatch,
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        publish_failure(
            &self.health,
            "cache owner task terminated (panic, abort or shutdown)".into(),
        );
    }
}

/// Await a command reply, racing it against the persistent terminal
/// failure watch. A failed reply send is also published sticky.
pub(crate) async fn await_reply<T>(
    rx: oneshot::Receiver<DbResult<T>>,
    mut health: watch::Receiver<Option<String>>,
    health_tx: HealthWatch,
) -> DbResult<T> {
    // Check the watch BEFORE the select so a ready reply cannot race it.
    if let Some(msg) = health.borrow().clone() {
        return Err(storage_err(msg));
    }
    tokio::select! {
        r = rx => match r {
            Ok(v) => {
                // Re-check health before returning a trusted success.
                if let Some(msg) = health.borrow_and_update().clone() {
                    return Err(storage_err(msg));
                }
                v
            }
            Err(_) => {
                let msg = "cache owner dropped the command reply".to_string();
                publish_failure(&health_tx, msg.clone());
                Err(storage_err(msg))
            }
        },
        changed = health.changed() => match changed {
            Ok(()) => Err(storage_err(
                health
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| "cache owner terminated".into()),
            )),
            Err(_) => Err(storage_err("cache owner health channel closed")),
        },
    }
}
