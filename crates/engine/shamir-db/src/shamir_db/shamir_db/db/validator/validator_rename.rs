//! Validator rename: direct, admitted (ordinary wire) and migration-owned
//! (proof-carrying) entry points. The rename body proper lives in
//! `ShamirDb::rename_validator_locked`; the admission modes differ only in
//! the exclusion/gate handling that precedes it.

use crate::access::Actor;
use crate::{DbError, DbResult};

use crate::shamir_db::shamir_db::ShamirDb;
use crate::shamir_db::shamir_db::ValidatorRenameMigration;

use super::validator_management::{ValidatorRegistrationError, ValidatorTargetAdmission};

impl ShamirDb {
    /// Rename a validator. The underlying WASM module is not recompiled.
    /// Id and bindings are unchanged.
    pub async fn rename_validator(&self, from: &str, to: &str) -> DbResult<()> {
        self.rename_validator_as(from, to, Actor::System).await
    }

    /// Like [`rename_validator`] but with an explicit [`Actor`].
    pub async fn rename_validator_as(&self, from: &str, to: &str, _actor: Actor) -> DbResult<()> {
        self.rename_validator_admitted(from, to, ValidatorTargetAdmission::ExclusionOnly)
            .await
            .map_err(|e| e.into_db_error("rename"))
    }

    /// Admission-carrying rename (review finding 3): rename re-keys the
    /// durable catalogue record of the ID bound tables invoke, so every
    /// binding holder takes the same target admission as replace. One
    /// replication-control acquisition spans the authoritative binding
    /// snapshot, the reservations, the registry name swap and the durable
    /// re-key.
    pub(in crate::shamir_db) async fn rename_validator_admitted(
        &self,
        from: &str,
        to: &str,
        admission: ValidatorTargetAdmission,
    ) -> Result<(), ValidatorRegistrationError> {
        let control = self.lock_replication_control().await;
        // Namespace gate (brief 84): every mode refuses while a
        // validator-rename migration is in progress (direct is a
        // conservative conflict; the migration itself goes through
        // `rename_validator_for_migration`).
        self.refuse_concurrent_validator_namespace_mutation("rename")?;
        let _guards = match admission {
            ValidatorTargetAdmission::ExclusionOnly => None,
            mode => {
                let exempt = match mode {
                    ValidatorTargetAdmission::FenceExcept(exempt) => exempt,
                    _ => None,
                };
                // AUTHORITATIVE (round 3, findings 1+2): see the locked
                // registration commit — the holder set comes from the
                // tables' own persisted bindings, not the reverse index.
                let dbs = match self.validators.id_for_name(from) {
                    Some(id) => self
                        .validator_binding_holders(&id, &control)
                        .await?
                        .dbs()
                        .to_vec(),
                    // `validators.rename` refuses the absent name below.
                    None => Vec::new(),
                };
                Some(self.reserve_validator_target_identities_locked(&dbs, exempt, &control)?)
            }
        };

        self.rename_validator_locked(from, to, &control).await
    }

    /// Migration-owned rename (brief 84 item 3): the caller proves gate
    /// ownership with the preflight's `ValidatorRenameMigration` guard, so
    /// the namespace gate is NOT re-checked — the guard IS the exemption.
    /// Every CURRENT binding-holder identity is verified against the proof
    /// (`permits()`) BEFORE the re-key; a holder added after the preflight
    /// fails closed instead of executing unchecked, and permitted holders
    /// are never re-reserved (the guard already holds their fences).
    /// Replication control is (re)acquired so the derivation and the
    /// registry/durable swap serialise against bind/unbind and the
    /// ordinary mutations.
    pub(in crate::shamir_db) async fn rename_validator_for_migration(
        &self,
        from: &str,
        to: &str,
        proof: &ValidatorRenameMigration<'_>,
    ) -> Result<(), ValidatorRegistrationError> {
        let control = self.lock_replication_control().await;
        let (foreign, serving) = proof.permits();
        if let Some(id) = self.validators.id_for_name(from) {
            let holders = self.validator_binding_holders(&id, &control).await?;
            for db in holders.dbs() {
                let Some(holder_id) = self.database_identity_for_fence(db) else {
                    // Stale binding ref: fail closed (same shape as the
                    // shared reservation helper).
                    return Err(ValidatorRegistrationError::Db(DbError::NotFound(format!(
                        "Database '{db}' not found"
                    ))));
                };
                if !foreign.contains(&holder_id) && Some(holder_id) != serving {
                    return Err(ValidatorRegistrationError::Db(DbError::Validation(
                        format!(
                            "cannot rename validator '{from}': database '{db}' bound it after \
                         the migration preflight reserved its targets"
                        ),
                    )));
                }
            }
        }
        self.rename_validator_locked(from, to, &control).await
    }

    /// The rename body proper (admission mode already applied by the
    /// caller): re-key the durable catalogue record and swap the live
    /// registry name — under the caller's replication control.
    async fn rename_validator_locked(
        &self,
        from: &str,
        to: &str,
        _control: &crate::shamir_db::shamir_db::ReplicationControlGuard<'_>,
    ) -> Result<(), ValidatorRegistrationError> {
        // Load the old catalogue record to re-key it.
        let old_record = self.system_store.load_validator(from).await?;

        // Rename in the live registry first.
        self.validators
            .rename(from, to)
            .map_err(|e| DbError::Validation(e.to_string()))?;

        // If there was a durable record, re-key it, preserving the
        // existing owner/group/mode. Write the durable new record BEFORE
        // removing the old one (a crash between the two leaves the new row
        // resolvable and the old row inert).
        if let Some(mut rec) = old_record {
            let existing_meta = crate::access::ResourceMeta::from_record(&rec);
            if let shamir_types::types::value::QueryValue::Map(ref mut map) = rec {
                map.insert(
                    "name".to_string(),
                    shamir_types::types::value::QueryValue::Str(to.to_string()),
                );
            }
            self.system_store
                .save_validator(to, &rec, &existing_meta)
                .await?;
            self.system_store.remove_validator(from).await?;
        }

        Ok(())
    }
}
