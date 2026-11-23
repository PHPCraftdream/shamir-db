//! Writer-level latch regression: once any append fails, the writer must
//! latch so `checkpoint_now` cannot certify a log missing that event.

use crate::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry, AuditError,
};
use std::sync::Arc;

fn key() -> [u8; 32] {
    [0xa1u8; 32]
}

#[derive(Default)]
struct CountingAppender {
    appends: std::sync::Mutex<Vec<AuditEntry>>,
    checkpoints: std::sync::Mutex<Vec<(u64, [u8; 32])>>,
}

impl AuditAppender for CountingAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.appends.lock().unwrap().push(entry.clone());
        Ok(())
    }
    fn checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]) -> Result<(), std::io::Error> {
        self.checkpoints
            .lock()
            .unwrap()
            .push((next_seq, *prev_hmac));
        Ok(())
    }
}

/// Only [`AuditError::SequenceExhausted`] used to latch the writer; every
/// other refusal was returned without latching, so `checkpoint_now`
/// certified a log missing that event. The refusal below never reaches the
/// appender and never sets `persistence_failed`, so with a healthy
/// appender only the latch can make `checkpoint_now` fail. Mutant: revert
/// the `Err(other)` arm to `return Err(other)` without the store — this
/// test then sees `Ok` from `checkpoint_now` (the parent commit's bug).
#[test]
fn writer_latches_any_append_failure_not_only_exhaustion() {
    let shared = Arc::new(AuditChain::new(key()));
    let concrete = Arc::new(CountingAppender::default());
    let appender = Arc::clone(&concrete) as Arc<dyn AuditAppender>;
    let writer = AuditChainWriter::new_with_shared(Arc::clone(&shared), appender);
    // Under MAX_RECORD_BYTES, so the size preflight admits it; the event
    // column's u16 length prefix is what refuses it.
    let over_long_event = "e".repeat(usize::from(u16::MAX) + 1);
    assert!(matches!(
        writer.append(over_long_event, "t", "x", "", [0; 8], "ok", Vec::new(), 0),
        Err(AuditError::FieldTooLong { field: "event" })
    ));
    assert!(concrete.appends.lock().unwrap().is_empty());
    assert!(concrete.checkpoints.lock().unwrap().is_empty());
    // The chain never consumed a sequence, so the log has no gap; the
    // refused event is simply absent, which is exactly what a checkpoint
    // must not certify.
    assert_eq!(writer.chain().checkpoint(), (1u64, [0; 32]));
    assert!(
        matches!(
            writer.checkpoint_now(),
            Err(AuditError::SequenceExhaustedUnaudited)
        ),
        "any append failure latches so an unaudited event cannot be certified"
    );
    assert!(concrete.checkpoints.lock().unwrap().is_empty());
}
