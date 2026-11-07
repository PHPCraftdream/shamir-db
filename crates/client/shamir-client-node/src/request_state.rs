use std::future::Future;
use std::sync::Arc;

use tokio::sync::{watch, RwLock};

pub(crate) type State<C> = Arc<RwLock<Option<C>>>;
pub(crate) type Operation<'a, T, E> =
    std::pin::Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

pub(crate) async fn request<C, T, E, F>(
    state: &State<C>,
    shutdown: &mut watch::Receiver<bool>,
    operation: F,
) -> Result<Result<T, E>, Closed>
where
    C: Send + Sync,
    F: for<'a> FnOnce(&'a C) -> Operation<'a, T, E>,
{
    let guard = tokio::select! {
        biased;
        changed = shutdown.changed() => {
            let _ = changed;
            return Err(Closed);
        }
        guard = state.read() => guard,
    };
    if *shutdown.borrow_and_update() {
        return Err(Closed);
    }
    let client = guard.as_ref().ok_or(Closed)?;
    tokio::select! {
        biased;
        changed = shutdown.changed() => {
            let _ = changed;
            Err(Closed)
        }
        result = operation(client) => Ok(result),
    }
}

pub(crate) struct Closed;
