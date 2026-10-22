//! B09: setup failure/cancellation must not strand MVCC publication.
//! The hook belongs immediately before unwrap/create setup (after the old
//! registration point). Restoring early registration strands an entry in
//! the first two tests and prevents init2 from completing in the race test.

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use std::sync::Arc;
use tokio::sync::Notify;

/// Per-instance, one-shot setup seam; Notify has one waiter per direction.
/// `notify_one` stores a permit even if the receiver has not polled yet.
pub(crate) struct TableInitHook {
    entered: Notify,
    resume: Notify,
    fail: bool,
}

impl TableInitHook {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            resume: Notify::new(),
            fail,
        })
    }

    pub(crate) async fn run(&self) -> DbResult<()> {
        if self.fail {
            return Err(DbError::Internal("B09 injected setup failure".into()));
        }
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(())
    }
}

fn make_repo() -> RepoInstance {
    RepoInstance::new(
        "retryable".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("orders")],
    )
}

fn assert_coherent(repo: &RepoInstance, table: &TableManager) {
    let mine = table.mvcc_store().expect("opened table must own MVCC");
    let registered = repo
        .per_table_mvcc()
        .read_sync(&table_token_for("orders"), |_, mvcc| Arc::clone(mvcc))
        .expect("opened table must be registered");
    assert!(Arc::ptr_eq(&mine, &registered));
}

fn assert_unregistered(repo: &RepoInstance) {
    assert!(repo
        .per_table_mvcc()
        .read_sync(&table_token_for("orders"), |_, _| ())
        .is_none());
}

#[tokio::test]
async fn setup_failure_retries_with_coherent_mvcc() {
    let repo = make_repo();
    repo.table_init_hook.store(Some(TableInitHook::new(true)));
    let error = match repo.get_table("orders").await {
        Err(error) => error,
        Ok(_) => panic!("injected setup failure must propagate"),
    };
    assert!(
        matches!(&error, DbError::Internal(message) if message == "B09 injected setup failure")
    );
    assert_unregistered(&repo);
    let table = repo.get_table("orders").await.unwrap();
    assert_eq!(table.name(), "orders");
    assert_coherent(&repo, &table);
    let again = repo.get_table("orders").await.unwrap();
    assert!(Arc::ptr_eq(
        &table.mvcc_store().unwrap(),
        &again.mvcc_store().unwrap(),
    ));
}

#[tokio::test]
async fn cancelled_setup_retries_with_coherent_mvcc() {
    let repo = make_repo();
    let hook = TableInitHook::new(false);
    repo.table_init_hook.store(Some(Arc::clone(&hook)));
    let worker_repo = repo.clone();
    let worker = tokio::spawn(async move { worker_repo.get_table("orders").await });
    hook.entered.notified().await;
    worker.abort();
    assert!(matches!(worker.await, Err(error) if error.is_cancelled()));
    assert_unregistered(&repo);
    let table = repo.get_table("orders").await.unwrap();
    assert_coherent(&repo, &table);
}

#[tokio::test]
async fn retired_initializer_cannot_disrupt_replacement() {
    let repo = make_repo();
    let contended = Arc::new(Notify::new());
    repo.write_domain()
        .install_contended_admission_for_test(Arc::clone(&contended));

    let hook = TableInitHook::new(false);
    repo.table_init_hook.store(Some(Arc::clone(&hook)));
    let worker_repo = repo.clone();
    let worker = tokio::spawn(async move { worker_repo.get_table("orders").await });
    hook.entered.notified().await;
    assert!(repo.remove_table_for_test("orders"));
    repo.add_table_for_test(TableConfig::new("orders"));
    let replacement_repo = repo.clone();
    let mut replacement = Box::pin(replacement_repo.get_table("orders"));
    assert!(futures::poll!(replacement.as_mut()).is_pending());
    contended.notified().await;
    hook.resume.notify_one();
    let result = worker.await.unwrap();
    assert!(matches!(
        result,
        Err(DbError::Internal(_)) | Err(DbError::NotFound(_))
    ));
    let replacement = replacement.await.unwrap();
    assert_coherent(&repo, &replacement);
    let current = repo.get_table("orders").await.unwrap();
    assert!(Arc::ptr_eq(
        &replacement.mvcc_store().unwrap(),
        &current.mvcc_store().unwrap(),
    ));
    assert!(repo.remove_table_for_test("orders"));
    assert_unregistered(&repo);
}

/// Red before B09: init resumes after DROP, returns a retired manager and
/// leaves its newly inserted MVCC registered despite missing config.
#[tokio::test]
async fn dropped_config_rolls_back_retired_publication() {
    let repo = make_repo();
    let hook = TableInitHook::new(false);
    repo.table_init_hook.store(Some(Arc::clone(&hook)));
    let worker_repo = repo.clone();
    let worker = tokio::spawn(async move { worker_repo.get_table("orders").await });
    hook.entered.notified().await;
    assert!(repo.remove_table_for_test("orders"));
    hook.resume.notify_one();
    assert!(matches!(worker.await.unwrap(), Err(DbError::NotFound(_))));
    assert_unregistered(&repo);
    repo.add_table_for_test(TableConfig::new("orders"));
    let table = repo.get_table("orders").await.unwrap();
    assert_coherent(&repo, &table);
}

/// DROP of an uninitialized cell must not erase somebody else's attachment.
/// Red before B09: remove_table unconditionally removes the token's MVCC.
#[tokio::test]
async fn remove_empty_cell_preserves_foreign_mvcc() {
    let repo = make_repo();
    let gate = repo.tx_gate().await.unwrap();
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let foreign = Arc::new(shamir_tx::MvccStore::new(history, gate));
    let token = table_token_for("orders");
    assert!(repo
        .per_table_mvcc()
        .insert_sync(token, Arc::clone(&foreign))
        .is_ok());
    assert!(matches!(
        repo.get_table("orders").await,
        Err(DbError::Internal(_))
    ));
    assert!(repo.remove_table_for_test("orders"));
    let retained = repo
        .per_table_mvcc()
        .read_sync(&token, |_, mvcc| Arc::clone(mvcc))
        .expect("foreign registration must survive DROP");
    assert!(Arc::ptr_eq(&retained, &foreign));
}
