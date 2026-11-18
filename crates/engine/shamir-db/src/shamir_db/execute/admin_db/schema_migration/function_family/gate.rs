//! In-process server-wide serialization for function-family migrations.
//!
//! Clones of one engine share the flag; independent engines do not. This
//! does not gate standalone admin edits. On restart the flag starts clear:
//! durable fences and in-doubt markers remain per database, not server-wide.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::query::batch::BatchError;

use super::super::plan::coded;

pub(in crate::shamir_db::execute::admin_db_schema_migration) struct FunctionFamilyGate<'a>(
    &'a AtomicBool,
);

impl<'a> FunctionFamilyGate<'a> {
    pub(in crate::shamir_db::execute::admin_db_schema_migration) fn acquire(
        flag: &'a AtomicBool,
    ) -> Result<Self, BatchError> {
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| {
                coded(
                    "schema_migration_in_progress",
                    "another function-family migration is running on this server".to_string(),
                )
            })?;
        Ok(Self(flag))
    }
}

impl Drop for FunctionFamilyGate<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
