//! Validator DROP: direct wrapper (`ExclusionOnly`) and the admitted wire
//! path. `mod.rs` re-exports only; one primary item per file.

use crate::access::Actor;
use crate::shamir_db::shamir_db::ShamirDb;
use crate::{DbError, DbResult};

use super::validator_management::{ValidatorRegistrationError, ValidatorTargetAdmission};

impl ShamirDb {
    /// Drop a validator by name.
    ///
    /// Returns `Ok(true)` if the validator existed and was removed.
    /// Returns `Err` if the validator is still bound to tables.
    pub async fn drop_validator(&self, name: &str) -> DbResult<bool> {
        self.drop_validator_as(name, Actor::System).await
    }

    /// Like [`drop_validator`] but with an explicit [`Actor`]. Direct
    /// embedder API: exclusion only (`ExclusionOnly`), no fence admission —
    /// the wire path goes through [`drop_validator_admitted`]. The
    /// replication-control lock closes the race where a bind makes the
    /// validator bound between the bound-refusal check and the removal
    /// (review finding 3 follow-up).
    pub async fn drop_validator_as(&self, name: &str, actor: Actor) -> DbResult<bool> {
        self.drop_validator_admitted(name, actor, ValidatorTargetAdmission::ExclusionOnly)
            .await
            .map_err(|e| e.into_db_error("drop"))
    }

    /// Admitted drop (wire path): derive the binding holders
    /// AUTHORITATIVELY (round 3, findings 1+2) from the tables' own
    /// persisted bindings. When bound, the holders' migration fences are
    /// reserved FIRST so a fenced holder returns its exact stable refusal
    /// code through the handler; the drop itself still refuses with the
    /// previous still-bound message (the reservation is released on
    /// return — it exists only to surface the fence, not to permit the
    /// drop). `ExclusionOnly` skips the reservation, mirroring the other
    /// direct embedder mutations.
    pub(in crate::shamir_db) async fn drop_validator_admitted(
        &self,
        name: &str,
        _actor: Actor,
        admission: ValidatorTargetAdmission,
    ) -> Result<bool, ValidatorRegistrationError> {
        let _control = self.lock_replication_control().await;
        // Namespace gate (brief 84): drop refuses while a validator-rename
        // migration is in progress — every mode, including direct
        // `ExclusionOnly` (a conservative conflict).
        self.refuse_concurrent_validator_namespace_mutation("drop")?;
        let id = match self.validators.id_for_name(name) {
            Some(id) => id,
            None => return Ok(false),
        };

        let holders = self
            .validator_binding_holders(&id, &_control)
            .await
            .map_err(ValidatorRegistrationError::Db)?;
        if !holders.is_empty() {
            if !matches!(admission, ValidatorTargetAdmission::ExclusionOnly) {
                // Fail-fast in the sorted holder order; a fenced holder's
                // exact code propagates as `Fence` through the handler.
                let _admission_guards = self.reserve_validator_target_identities_locked(
                    holders.dbs(),
                    None,
                    &_control,
                )?;
            }
            // Round 6: a holder whose binding record is IN DOUBT is evidence
            // without a name — it holds every validator identity, so the drop
            // is refused, but its table ref must not reach the message (the
            // fourth review's foreign-table disclosure stays closed).
            let bound = if holders.tables().is_empty() {
                "binding state in doubt (restart required)".to_string()
            } else {
                holders.tables().join(", ")
            };
            return Err(DbError::Validation(format!(
                "cannot drop validator '{}': still bound to tables: {}",
                name, bound
            ))
            .into());
        }

        // Durable removal FIRST (RV-DB-6540DC7B): a persistence error on
        // this PREWRITE delete propagates and leaves the live registry
        // binding intact, so a retry still sees the validator. (A remove
        // that commits and then fails at flush can also surface here; no
        // restart-visibility guarantee is claimed for that case.) No
        // durable row — the delete matches nothing and succeeds
        // idempotently.
        self.system_store
            .remove_validator(name)
            .await
            .map_err(ValidatorRegistrationError::Db)?;
        let existed = self.validators.remove(&id);
        Ok(existed)
    }
}
