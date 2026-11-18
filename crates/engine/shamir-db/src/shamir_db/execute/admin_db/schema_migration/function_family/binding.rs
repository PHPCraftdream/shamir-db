//! Recovery evidence for table-local validator bindings. Bind can be inspected
//! but cannot be replayed: admission cannot quiesce already staged writes.

use std::sync::Arc;

use futures::StreamExt;
use shamir_engine::meta::{MetaEnvelope, MetaKey};
use shamir_engine::table::TableManager;
use shamir_engine::validator::{PersistedValidators, ValidatorBinding, WriteOp};
use shamir_funclib::canonical::canonical_hash;
use shamir_query_types::admin::types::schema_migration::SchemaMigrationOp;
use shamir_types::types::record_id::RecordId;

use crate::query::batch::BatchError;
use crate::types::value::QueryValue;

use super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::plan::coded;
use super::{admission, registration};

struct Target<'a> {
    repo: &'a str,
    table: &'a str,
    validator: &'a str,
    bind: Option<(&'a [WriteOp], u16)>,
}

struct Observation {
    identity: String,
    binding: Option<String>,
    saved: bool,
    matches_effect: bool,
}

pub(super) async fn capture_intent(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
) -> Result<String, BatchError> {
    let target = target(executor, operation)?;
    let observed = observe(executor, &target, false).await?;
    let original = match observed.binding.as_deref() {
        Some(binding) => binding,
        None if observed.saved => "absent",
        None => "never-saved",
    };
    Ok(format!("binding-v1:{}:{original}", observed.identity))
}

pub(super) async fn verify(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
    intent: Option<&str>,
) -> Result<String, BatchError> {
    let target = target(executor, operation)?;
    let (identity, original) = parse_intent(intent)?;
    let observed = observe(executor, &target, true).await?;
    check_identity(&observed, identity)?;
    if effect_proven(&observed, original) {
        Ok(effect_evidence(&observed))
    } else {
        Err(postcondition(
            "intended validator binding metadata not proven",
        ))
    }
}

pub(super) async fn reconcile(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
    intent: Option<&str>,
) -> Result<Option<String>, BatchError> {
    let target = target(executor, operation)?;
    let (identity, original) = parse_intent(intent)?;
    let observed = observe(executor, &target, true).await?;
    check_identity(&observed, identity)?;
    if effect_proven(&observed, original) {
        return Ok(Some(effect_evidence(&observed)));
    }
    // Only Unbind can replay, and only against the exact original binding.
    if target.bind.is_none() && observed.binding.as_deref() == Some(original) {
        return Ok(None);
    }
    Err(postcondition(
        "neither the intended effect nor a replayable original binding was proven",
    ))
}

fn target<'a>(
    executor: &ShamirAdminExecutor,
    operation: &'a SchemaMigrationOp,
) -> Result<Target<'a>, BatchError> {
    admission::validate_binding_scope(&executor.db_name, operation)?;
    match operation {
        SchemaMigrationOp::BindValidator(op) => {
            if op.ops.is_empty() || !(1000..=9999).contains(&op.priority) {
                return Err(postcondition("invalid validator binding ops or priority"));
            }
            Ok(Target {
                repo: &op.repo,
                table: &op.table,
                validator: &op.bind_validator,
                bind: Some((&op.ops, op.priority)),
            })
        }
        SchemaMigrationOp::UnbindValidator(op) => Ok(Target {
            repo: &op.repo,
            table: &op.table,
            validator: &op.unbind_validator,
            bind: None,
        }),
        _ => Err(postcondition("expected a validator binding operation")),
    }
}

async fn observe(
    executor: &ShamirAdminExecutor,
    target: &Target<'_>,
    make_durable: bool,
) -> Result<Observation, BatchError> {
    let db = &executor.db_name;
    let shamir = &executor.shamir;
    let registry = shamir.validators();
    let id = registry
        .id_for_name(target.validator)
        .ok_or_else(|| postcondition("target validator is not registered"))?;
    let artifact = registry
        .get_by_id(&id)
        .ok_or_else(|| postcondition("target validator artifact is missing"))?;
    let registration = registration::validator(executor, target.validator)
        .await?
        .ok_or_else(|| postcondition("target validator catalogue row is missing"))?;
    let table = shamir
        .get_table(db, target.repo, target.table)
        .await
        .map_err(|_| postcondition("target validator binding table is unavailable"))?;
    let system = shamir.system_store();
    let repo = shamir
        .get_db(db)
        .and_then(|instance| instance.get_repo(target.repo))
        .ok_or_else(|| postcondition("target validator binding repository is unavailable"))?;
    let store_id = repo
        .table_config(target.table)
        .and_then(|config| config.store_id)
        .ok_or_else(|| postcondition("target table has no stable store identity"))?;
    if !repo.has_persistent_wal() {
        return Err(durability_error());
    }
    if make_durable {
        // `flush_metadata` only drains the interner/counter persist registry.
        // Bindings are written directly to the info store. Also sync the
        // catalogue: the handler's best-effort bound_in update hides errors.
        repo.sync_wal().await.map_err(|_| durability_error())?;
        table
            .info_store_read_view()
            .flush()
            .await
            .map_err(|_| durability_error())?;
        let system_repo = system.system_repo().map_err(|_| durability_error())?;
        let validators = system_repo
            .get_table("validators")
            .await
            .map_err(|_| durability_error())?;
        system
            .flush_system_repo_durable(&validators)
            .await
            .map_err(|_| durability_error())?;
    }

    let live = table.validator_bindings();
    let persisted = read_bindings(&table).await?;
    let binding = find_binding(&live, &id)?;
    let stored = find_binding(
        persisted
            .as_ref()
            .map_or(&[], |state| state.bindings.as_slice()),
        &id,
    )?;
    if !same_binding(binding, stored) {
        return Err(postcondition(
            "live and stored validator binding metadata disagree",
        ));
    }

    // registration::validator has already checked uniqueness, row shape,
    // and the catalogue ID. Read the mutable reverse reference separately.
    let row = system
        .load_validator(target.validator)
        .await
        .map_err(|_| postcondition("validator catalogue reverse references are unreadable"))?
        .ok_or_else(|| postcondition("validator catalogue row disappeared"))?;
    let row_id = row
        .get("_id")
        .and_then(QueryValue::as_str)
        .and_then(|text| text.parse::<RecordId>().ok());
    let table_ref = format!("{db}/{}/{}", target.repo, target.table);
    let Some(QueryValue::List(refs)) = row.get("bound_in") else {
        return Err(postcondition(
            "validator catalogue bound_in metadata is missing or invalid",
        ));
    };
    let mut count = 0;
    for entry in refs {
        let entry = entry
            .as_str()
            .ok_or_else(|| postcondition("invalid bound_in reference"))?;
        if entry == table_ref {
            count += 1;
        }
    }
    let registry_bound = registry.bound_tables(&id).contains(&table_ref);
    if row_id != Some(id)
        || registry.id_for_name(target.validator) != Some(id)
        || !registry
            .get_by_id(&id)
            .is_some_and(|current| Arc::ptr_eq(&current, &artifact))
        || !Arc::ptr_eq(&live, &table.validator_bindings())
        || count != usize::from(binding.is_some())
        || registry_bound != binding.is_some()
    {
        return Err(postcondition(
            "validator binding and catalogue reverse references disagree",
        ));
    }

    let identity = canonical_hash(&QueryValue::List(vec![
        QueryValue::Str("schema-migration-binding-target-v1".into()),
        QueryValue::Str(db.clone()),
        QueryValue::Str(target.repo.into()),
        QueryValue::Str(target.table.into()),
        QueryValue::Str(store_id.to_string()),
        QueryValue::Str(target.validator.into()),
        QueryValue::Str(id.to_string()),
        QueryValue::Str(registration.fingerprint()?),
    ]))
    .map_err(|_| postcondition("could not fingerprint validator binding target"))?;
    let matches_effect = match (target.bind, binding) {
        (None, None) => true,
        (Some((ops, priority)), Some(binding)) => {
            binding.ops.as_slice() == ops && binding.priority == priority
        }
        _ => false,
    };
    Ok(Observation {
        identity,
        binding: binding.map(binding_fingerprint).transpose()?,
        saved: persisted.is_some(),
        matches_effect,
    })
}

async fn read_bindings(table: &TableManager) -> Result<Option<PersistedValidators>, BatchError> {
    let key = MetaKey::Validators.as_record_id();
    let view = table.info_store_read_view();
    let mut batches = view.scan_prefix_stream(key.as_bytes().to_vec().into(), 16);
    let mut persisted = None;
    while let Some(batch) = batches.next().await {
        for (found, data) in
            batch.map_err(|_| postcondition("validator binding metadata read failed"))?
        {
            if found.as_ref() != key.as_bytes() {
                continue;
            }
            if persisted.is_some() {
                return Err(postcondition("duplicate validator binding metadata"));
            }
            persisted =
                Some(MetaEnvelope::open(&data).map_err(|_| {
                    postcondition("validator binding metadata could not be decoded")
                })?);
        }
    }
    Ok(persisted)
}

fn find_binding<'a>(
    bindings: &'a [ValidatorBinding],
    id: &RecordId,
) -> Result<Option<&'a ValidatorBinding>, BatchError> {
    let mut matching = bindings.iter().filter(|entry| entry.validator_id == *id);
    let entry = matching.next();
    if matching.next().is_some() {
        return Err(postcondition("duplicate bindings for the target validator"));
    }
    Ok(entry)
}

fn same_binding(live: Option<&ValidatorBinding>, stored: Option<&ValidatorBinding>) -> bool {
    match (live, stored) {
        (None, None) => true,
        (Some(live), Some(stored)) => live.ops == stored.ops && live.priority == stored.priority,
        _ => false,
    }
}

fn binding_fingerprint(binding: &ValidatorBinding) -> Result<String, BatchError> {
    let ops = binding
        .ops
        .iter()
        .map(|op| {
            QueryValue::Str(
                match op {
                    WriteOp::Insert => "insert",
                    WriteOp::Update => "update",
                    WriteOp::Upsert => "upsert",
                    WriteOp::Delete => "delete",
                }
                .into(),
            )
        })
        .collect();
    canonical_hash(&QueryValue::List(vec![
        QueryValue::Str("schema-migration-binding-v1".into()),
        QueryValue::Str(binding.validator_id.to_string()),
        QueryValue::Int(i64::from(binding.priority)),
        QueryValue::List(ops),
    ]))
    .map_err(|_| postcondition("could not fingerprint original validator binding"))
}

fn parse_intent(intent: Option<&str>) -> Result<(&str, &str), BatchError> {
    intent
        .and_then(|intent| intent.strip_prefix("binding-v1:"))
        .and_then(|intent| intent.split_once(':'))
        .filter(|(identity, original)| {
            is_fingerprint(identity)
                && (matches!(*original, "absent" | "never-saved") || is_fingerprint(original))
        })
        .ok_or_else(|| postcondition("missing or malformed validator binding intent evidence"))
}

fn is_fingerprint(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn check_identity(observed: &Observation, identity: &str) -> Result<(), BatchError> {
    if observed.identity != identity {
        return Err(postcondition("validator binding target identity changed"));
    }
    Ok(())
}

fn effect_proven(observed: &Observation, original: &str) -> bool {
    // Removing a binding writes a metadata envelope even for the last
    // binding. A missing envelope is not evidence of a completed removal.
    observed.matches_effect && (observed.saved || original == "never-saved")
}

fn effect_evidence(observed: &Observation) -> String {
    let result = observed.binding.as_deref().unwrap_or("absent");
    format!("binding-v1:{}:{result}", observed.identity)
}

fn postcondition(message: &str) -> BatchError {
    coded("schema_migration_postcondition_failed", message.into())
}

fn durability_error() -> BatchError {
    coded(
        "schema_migration_durability_unavailable",
        "could not establish validator binding durability".into(),
    )
}
