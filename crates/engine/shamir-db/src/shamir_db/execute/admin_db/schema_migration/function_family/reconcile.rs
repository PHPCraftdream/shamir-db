//! Recovery decisions for admitted function and validator operations.
//!
//! | Operation | Required intent | Pending: retry | Pending: complete / verify |
//! | --- | --- | --- | --- |
//! | Create function or validator | Name absent | Still absent | Exact requested registration |
//! | Rename function or validator | Source fingerprint, destination absent | Exact source, destination absent | Source absent, exact destination |
//! | Alter function | Invariant fingerprint and old optimize | Same invariants and old optimize only | Same invariants and requested optimize |
//! | Check function | No effect | Always | Fixed no-effect evidence after successful check |
//! | Drop function or validator | Source fingerprint | Exact source | Source absent |
//! | Bind or unbind validator | Binding-specific intent | Delegated | Delegated |
//!
//! Anything not proven by these rows fails closed. Migration admission owns
//! HMAC refusal; these fingerprints are not authorization proofs.

use shamir_query_types::admin::types::schema_migration::SchemaMigrationOp;

#[cfg(test)]
use crate::access::Actor;
use crate::query::batch::BatchError;
#[cfg(test)]
use crate::shamir_db::ShamirDb;

use super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::plan::coded;
use super::registration::{self, Registration};
use super::{admission, binding};

const ABSENT: &str = "pre_state=absent";
const PRE_STATE: &str = "pre_state=";
const DESTINATION_ABSENT: &str = ";destination=absent";
const OPTIMIZE: &str = ";optimize=";
const NO_EFFECT: &str = "check_function=no_effect";

pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn capture_intent(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
) -> Result<String, BatchError> {
    Box::pin(admission::authorize_family_operation(executor, operation)).await?;
    match operation {
        SchemaMigrationOp::CreateFunction(op) => {
            absent_intent(Box::pin(registration::function(executor, &op.create_function)).await?)
        }
        SchemaMigrationOp::CreateValidator(op) => {
            absent_intent(Box::pin(registration::validator(executor, &op.create_validator)).await?)
        }
        SchemaMigrationOp::DropFunction(op) => {
            source_intent(Box::pin(registration::function(executor, &op.drop_function)).await?)
        }
        SchemaMigrationOp::DropValidator(op) => {
            source_intent(Box::pin(registration::validator(executor, &op.drop_validator)).await?)
        }
        SchemaMigrationOp::RenameFunction(op) => {
            let source = Box::pin(registration::function(executor, &op.rename_function)).await?;
            let destination = Box::pin(registration::function(executor, &op.to)).await?;
            rename_intent(source, destination)
        }
        SchemaMigrationOp::RenameValidator(op) => {
            let source = Box::pin(registration::validator(executor, &op.rename_validator)).await?;
            let destination = Box::pin(registration::validator(executor, &op.to)).await?;
            rename_intent(source, destination)
        }
        SchemaMigrationOp::AlterFunction(op) => {
            let prior = Box::pin(registration::function(executor, &op.alter_function))
                .await?
                .ok_or_else(|| failed("alter source registration is absent"))?;
            let previous = prior
                .optimize()
                .ok_or_else(|| failed("alter source optimize mode is missing"))?;
            let invariant = prior.fingerprint_without_optimize()?;
            Ok(format!("{PRE_STATE}{invariant}{OPTIMIZE}{previous}"))
        }
        SchemaMigrationOp::CheckFunction(_) => Ok(NO_EFFECT.into()),
        SchemaMigrationOp::BindValidator(_) | SchemaMigrationOp::UnbindValidator(_) => {
            Box::pin(binding::capture_intent(executor, operation)).await
        }
        _ => Err(failed("not a function-family operation")),
    }
}

pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn verify(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
    intent: Option<&str>,
) -> Result<String, BatchError> {
    match operation {
        SchemaMigrationOp::BindValidator(_) | SchemaMigrationOp::UnbindValidator(_) => {
            Box::pin(admission::authorize_family_operation(executor, operation)).await?;
            Box::pin(binding::verify(executor, operation, intent)).await
        }
        SchemaMigrationOp::CheckFunction(_) => {
            Box::pin(admission::authorize_family_operation(executor, operation)).await?;
            require_intent(intent, NO_EFFECT)?;
            Ok(NO_EFFECT.into())
        }
        _ => Box::pin(reconcile(executor, operation, intent))
            .await?
            .ok_or_else(|| failed("registration still has the intent pre-state")),
    }
}

pub(in crate::shamir_db::execute::admin_db_schema_migration) async fn reconcile(
    executor: &ShamirAdminExecutor,
    operation: &SchemaMigrationOp,
    intent: Option<&str>,
) -> Result<Option<String>, BatchError> {
    Box::pin(admission::authorize_family_operation(executor, operation)).await?;
    match operation {
        SchemaMigrationOp::CreateFunction(op) => {
            require_intent(intent, ABSENT)?;
            classify_create(
                Box::pin(registration::function(executor, &op.create_function)).await?,
                |current| current.matches_function(op),
            )
        }
        SchemaMigrationOp::CreateValidator(op) => {
            require_intent(intent, ABSENT)?;
            classify_create(
                Box::pin(registration::validator(executor, &op.create_validator)).await?,
                |current| current.matches_validator(op),
            )
        }
        SchemaMigrationOp::DropFunction(op) => classify_drop(
            fingerprint_intent(intent, "")?,
            Box::pin(registration::function(executor, &op.drop_function)).await?,
        ),
        SchemaMigrationOp::DropValidator(op) => classify_drop(
            fingerprint_intent(intent, "")?,
            Box::pin(registration::validator(executor, &op.drop_validator)).await?,
        ),
        SchemaMigrationOp::RenameFunction(op) => {
            // Gate diagnostics on both endpoints. An absent endpoint has open
            // default metadata, not proof of authority: completion requires the
            // exact destination, and replay rechecks the actual source ACL.
            Box::pin(admission::authorize_proven_rename(executor, operation)).await?;
            let expected = fingerprint_intent(intent, DESTINATION_ABSENT)?;
            let source = Box::pin(registration::function(executor, &op.rename_function)).await?;
            let destination = Box::pin(registration::function(executor, &op.to)).await?;
            classify_rename(expected, source, destination)
        }
        SchemaMigrationOp::RenameValidator(op) => {
            let expected = fingerprint_intent(intent, DESTINATION_ABSENT)?;
            let source = Box::pin(registration::validator(executor, &op.rename_validator)).await?;
            let destination = Box::pin(registration::validator(executor, &op.to)).await?;
            classify_rename(expected, source, destination)
        }
        SchemaMigrationOp::AlterFunction(op) => {
            let (invariant, previous) = intent
                .and_then(|text| text.split_once(OPTIMIZE))
                .filter(|(_, mode)| matches!(*mode, "auto" | "now" | "never"))
                .ok_or_else(|| failed("alter intent lacks a valid previous optimize mode"))?;
            let expected = fingerprint_intent(Some(invariant), "")?;
            let current = Box::pin(registration::function(executor, &op.alter_function))
                .await?
                .ok_or_else(|| failed("alter registration is absent"))?;
            if current.fingerprint_without_optimize()? != expected {
                Err(failed("alter registration invariants changed"))
            } else if current.optimize() == Some(op.optimize.as_str()) {
                current.fingerprint().map(Some)
            } else if current.optimize() == Some(previous) {
                Ok(None)
            } else {
                Err(failed(
                    "alter optimize mode matches neither intent nor request",
                ))
            }
        }
        SchemaMigrationOp::CheckFunction(_) => {
            require_intent(intent, NO_EFFECT)?;
            Ok(None)
        }
        SchemaMigrationOp::BindValidator(_) | SchemaMigrationOp::UnbindValidator(_) => {
            Box::pin(binding::reconcile(executor, operation, intent)).await
        }
        _ => Err(failed("not a function-family operation")),
    }
}

fn absent_intent(prior: Option<Registration>) -> Result<String, BatchError> {
    if prior.is_some() {
        return Err(failed("create requires an absent registration"));
    }
    Ok(ABSENT.into())
}

fn source_intent(prior: Option<Registration>) -> Result<String, BatchError> {
    let prior = prior.ok_or_else(|| failed("source registration is absent"))?;
    Ok(format!("{PRE_STATE}{}", prior.fingerprint()?))
}

fn rename_intent(
    source: Option<Registration>,
    destination: Option<Registration>,
) -> Result<String, BatchError> {
    if destination.is_some() {
        return Err(failed("rename destination registration already exists"));
    }
    Ok(format!("{}{DESTINATION_ABSENT}", source_intent(source)?))
}

fn classify_create(
    current: Option<Registration>,
    matches_request: impl FnOnce(&Registration) -> Result<bool, BatchError>,
) -> Result<Option<String>, BatchError> {
    let Some(current) = current else {
        return Ok(None);
    };
    if !matches_request(&current)? {
        return Err(failed("created registration does not match the request"));
    }
    current.fingerprint().map(Some)
}

fn classify_drop(
    expected: &str,
    current: Option<Registration>,
) -> Result<Option<String>, BatchError> {
    match current {
        None => Ok(Some("registration=absent".into())),
        Some(current) if current.fingerprint()? == expected => Ok(None),
        Some(_) => Err(failed("drop source does not match the intent fingerprint")),
    }
}

fn classify_rename(
    expected: &str,
    source: Option<Registration>,
    destination: Option<Registration>,
) -> Result<Option<String>, BatchError> {
    match (source, destination) {
        (Some(source), None) if source.fingerprint()? == expected => Ok(None),
        (None, Some(destination)) if destination.fingerprint()? == expected => {
            Ok(Some(expected.into()))
        }
        _ => Err(failed(
            "rename endpoints do not prove an exact move or unchanged source",
        )),
    }
}

fn require_intent(intent: Option<&str>, expected: &str) -> Result<(), BatchError> {
    if intent == Some(expected) {
        Ok(())
    } else {
        Err(failed("missing or invalid operation intent evidence"))
    }
}

fn fingerprint_intent<'a>(intent: Option<&'a str>, suffix: &str) -> Result<&'a str, BatchError> {
    intent
        .and_then(|text| text.strip_prefix(PRE_STATE)?.strip_suffix(suffix))
        .filter(|hash| {
            hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
        .ok_or_else(|| failed("missing or invalid registration fingerprint intent"))
}

fn failed(message: &str) -> BatchError {
    coded("schema_migration_postcondition_failed", message.into())
}

#[cfg(test)]
pub(in crate::shamir_db) async fn test_capture_intent(
    shamir: &ShamirDb,
    db_name: &str,
    actor: Actor,
    operation: &SchemaMigrationOp,
) -> Result<String, BatchError> {
    let executor = ShamirAdminExecutor {
        shamir: shamir.clone(),
        db_name: db_name.into(),
        actor,
        repo_admin_observer: None,
    };
    Box::pin(capture_intent(&executor, operation)).await
}

#[cfg(test)]
pub(in crate::shamir_db) async fn test_verify(
    shamir: &ShamirDb,
    db_name: &str,
    actor: Actor,
    operation: &SchemaMigrationOp,
    intent: Option<&str>,
) -> Result<String, BatchError> {
    let executor = ShamirAdminExecutor {
        shamir: shamir.clone(),
        db_name: db_name.into(),
        actor,
        repo_admin_observer: None,
    };
    Box::pin(verify(&executor, operation, intent)).await
}
