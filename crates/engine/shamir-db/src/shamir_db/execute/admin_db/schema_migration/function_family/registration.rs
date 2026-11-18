//! Read-only catalogue evidence for function-family migration reconciliation.
//!
//! Callers own authorization and fencing. These fingerprints are not HMACs,
//! authorization decisions, or receipts for a live registry swap.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use shamir_engine::function::{
    admit_module, AdmissionPolicy, FunctionMeta, ModuleHash, Security, TierMode, Visibility,
};
use shamir_funclib::canonical::canonical_hash;
use shamir_query_builder::{filter, Query};
use shamir_query_types::admin::{CreateFunctionOp, CreateValidatorOp};
use shamir_types::types::record_id::RecordId;

use crate::access::ResourceMeta;
use crate::query::batch::BatchError;
use crate::types::common::new_map;
use crate::types::value::QueryValue;

use super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::plan::coded;

pub(super) struct Registration {
    lang: String,
    source: Option<String>,
    bytes: Vec<u8>,
    hash: ModuleHash,
    optimize: Option<&'static str>,
    function_meta: Option<FunctionMeta>,
    // Fixed-order fields that survive rename, not the physical row key/name.
    stable_metadata: Vec<QueryValue>,
}

impl Registration {
    pub(super) fn fingerprint(&self) -> Result<String, BatchError> {
        self.hash_fields(self.optimize)
    }

    pub(super) fn fingerprint_without_optimize(&self) -> Result<String, BatchError> {
        self.hash_fields(None)
    }

    fn hash_fields(&self, optimize: Option<&str>) -> Result<String, BatchError> {
        let meta = self.function_meta.as_ref().map_or(QueryValue::Null, |m| {
            QueryValue::List(vec![
                QueryValue::Str(m.visibility.to_string()),
                QueryValue::Str(m.security.to_string()),
                strings(&m.secret_grants),
                strings(&m.net_grants),
            ])
        });
        canonical_hash(&QueryValue::List(vec![
            QueryValue::Str("schema-migration-registration-v1".into()),
            QueryValue::Str(self.lang.clone()),
            text(self.source.as_deref()),
            QueryValue::Bin(self.bytes.clone()),
            QueryValue::Str(self.hash.to_hex()),
            QueryValue::List(self.stable_metadata.clone()),
            meta,
            text(optimize),
        ]))
        .map_err(|_| postcondition("registration fingerprint could not be encoded"))
    }

    pub(super) fn matches_function(&self, op: &CreateFunctionOp) -> Result<bool, BatchError> {
        let visibility = op
            .visibility
            .as_deref()
            .unwrap_or("private")
            .parse::<Visibility>()
            .map_err(|_| invalid_request())?;
        let security = op
            .security
            .as_deref()
            .unwrap_or("invoker")
            .parse::<Security>()
            .map_err(|_| invalid_request())?;
        let optimize = op.optimize.as_deref().unwrap_or("auto");
        if tier_mode(optimize).is_none() {
            return Err(invalid_request());
        }
        let payload =
            self.matches_payload(op.source.as_deref(), op.wasm.as_deref(), op.lang.as_deref())?;
        Ok(payload
            && self.optimize() == Some(optimize)
            && self.function_meta.as_ref().is_some_and(|m| {
                m.visibility == visibility
                    && m.security == security
                    && m.secret_grants == op.secret_grants
                    && m.net_grants == op.net_grants
            }))
    }

    /// Exact stored payload only; an existing live validator with this ID
    /// may still hold old code after an interrupted replacement.
    pub(super) fn matches_validator(&self, op: &CreateValidatorOp) -> Result<bool, BatchError> {
        let payload = self.matches_payload(op.source.as_deref(), op.wasm.as_deref(), None)?;
        Ok(payload && self.function_meta.is_none())
    }

    pub(super) fn optimize(&self) -> Option<&str> {
        self.optimize
    }

    fn matches_payload(
        &self,
        source: Option<&str>,
        wasm: Option<&str>,
        lang: Option<&str>,
    ) -> Result<bool, BatchError> {
        match (source, wasm, lang) {
            (Some(source), None, lang @ (None | Some("rust" | "shamir"))) => {
                Ok(self.lang == lang.unwrap_or("rust") && self.source.as_deref() == Some(source))
            }
            (None, Some(wasm), None | Some("wasm")) => {
                let bytes = STANDARD.decode(wasm).map_err(|_| invalid_request())?;
                Ok(self.lang == "wasm" && self.bytes == bytes)
            }
            _ => Err(invalid_request()),
        }
    }
}

pub(super) async fn function(
    executor: &ShamirAdminExecutor,
    name: &str,
) -> Result<Option<Registration>, BatchError> {
    let registry = executor.shamir.functions();
    let before = registry.capture(name);
    let row = catalogue_row(executor, "functions", name).await?;
    let live_meta = executor.shamir.function_meta(name);
    let live_tier = registry
        .get(name)
        .and_then(|artifact| artifact.tier_report());
    let after = registry.capture(name);
    let (row, binding) = match (row, before, after, live_meta.as_ref()) {
        (None, None, None, None) => return Ok(None),
        (Some(row), Some(before), Some(after), Some(_)) if before.same_as(&after) => (row, after),
        _ => return Err(incoherent()),
    };

    let mut registration = stored_module(&row)?;
    let artifact_id = optional_string(&row, "artifact_id")?;
    if let Some(id) = artifact_id {
        id.parse::<RecordId>().map_err(|_| malformed())?;
    }
    let optimize = match optional_string(&row, "optimize")?.unwrap_or("auto") {
        "auto" => "auto",
        "now" => "now",
        "never" => "never",
        _ => return Err(malformed()),
    };
    let meta = FunctionMeta::try_from_record(&row).map_err(|_| malformed())?;
    let live_meta = live_meta.ok_or_else(incoherent)?;
    if binding.is_database_builtin()
        || binding.module_hash() != Some(registration.hash)
        || binding.artifact_id() != artifact_id
        || live_tier.map(|report| report.mode) != tier_mode(optimize)
        || meta.visibility != live_meta.visibility
        || meta.security != live_meta.security
        || meta.secret_grants != live_meta.secret_grants
        || meta.net_grants != live_meta.net_grants
    {
        return Err(incoherent());
    }

    let signature = match row.get("call_signature") {
        None => QueryValue::Null,
        Some(QueryValue::Bin(bytes)) if registration.lang == "shamir" && !bytes.is_empty() => {
            QueryValue::Bin(bytes.clone())
        }
        _ => return Err(malformed()),
    };
    let abi = optional_u32(&row, "abi")?;
    let version = optional_u32(&row, "version")?;
    registration.stable_metadata.extend([
        QueryValue::Str("function".into()),
        text(artifact_id),
        signature,
        abi,
        version,
        text(optional_string(&row, "origin")?),
    ]);
    registration.optimize = Some(optimize);
    registration.function_meta = Some(meta);
    Ok(Some(registration))
}

/// Validates persisted bytes and checks the live name/ID mapping and artifact
/// presence. `RecordValidator` does not expose the live module hash. In
/// particular, replacement reuses the ID, so a matching row is not proof of
/// a completed live replacement. Check bindings separately against the table
/// and registry; the mutable `bound_in` snapshot is not fingerprint evidence.
pub(super) async fn validator(
    executor: &ShamirAdminExecutor,
    name: &str,
) -> Result<Option<Registration>, BatchError> {
    let registry = executor.shamir.validators();
    let before = registry.id_for_name(name);
    let before_artifact = before.and_then(|id| registry.get_by_id(&id));
    let row = catalogue_row(executor, "validators", name).await?;
    let after = registry.id_for_name(name);
    let (row, live_id) = match (row, before, after) {
        (None, None, None) => return Ok(None),
        (Some(row), Some(before), Some(after)) if before == after => (row, after),
        _ => return Err(incoherent()),
    };
    let id = required_string(&row, "_id")?
        .parse::<RecordId>()
        .map_err(|_| malformed())?;
    let artifact = registry.get_by_id(&id).ok_or_else(incoherent)?;
    if id != live_id
        || registry.name_for_id(&id).as_deref() != Some(name)
        || !before_artifact.is_some_and(|before| Arc::ptr_eq(&before, &artifact))
    {
        return Err(incoherent());
    }
    if row.get("bound_in").is_some_and(|value| {
        !matches!(value, QueryValue::List(entries) if entries.iter().all(|entry| entry.as_str().is_some()))
    }) {
        return Err(malformed());
    }
    let mut registration = stored_module(&row)?;
    registration.stable_metadata.extend([
        QueryValue::Str("validator".into()),
        QueryValue::Str(id.to_string()),
    ]);
    // The live validator does not expose its module; independently reject
    // malformed catalogue modules without installing or invoking anything.
    // Do not run wasmparser on the async worker thread.
    let registration = tokio::task::spawn_blocking(move || {
        admit_module(&registration.bytes, &AdmissionPolicy::Load)
            .map_err(|_| postcondition("stored validator module failed structural validation"))?;
        Ok::<_, BatchError>(registration)
    })
    .await
    .map_err(|_| postcondition("stored validator module validation did not finish"))??;
    if registry.id_for_name(name) != Some(id)
        || !registry
            .get_by_id(&id)
            .is_some_and(|current| Arc::ptr_eq(&artifact, &current))
    {
        return Err(incoherent());
    }
    Ok(Some(registration))
}

/// The ordinary single-row loaders take the first match, and the validator
/// loader discards `corrupt_records`. Neither is sufficient for proving
/// absence or uniqueness here. Read the same system tables with those checks.
async fn catalogue_row(
    executor: &ShamirAdminExecutor,
    table_name: &str,
    name: &str,
) -> Result<Option<QueryValue>, BatchError> {
    let repo = executor
        .shamir
        .system_store()
        .system_repo()
        .map_err(|_| backend())?;
    let table = repo.get_table(table_name).await.map_err(|_| backend())?;
    let refs = new_map();
    let ctx = table
        .info_store_read_view()
        .filter_context(&refs)
        .await
        .map_err(|_| backend())?;
    let query = Query::from(table_name)
        .where_(filter::eq("name", name))
        .build();
    let result = table.read(&query, &ctx).await.map_err(|_| backend())?;
    if !result.corrupt_records.is_empty() || result.records.len() > 1 {
        return Err(malformed());
    }
    let row = result
        .records
        .into_iter()
        .next()
        .map(|r| r.as_value().into_owned());
    match row {
        Some(row) if required_string(&row, "name")? != name => Err(malformed()),
        row => Ok(row),
    }
}

fn stored_module(row: &QueryValue) -> Result<Registration, BatchError> {
    if !matches!(optional_string(row, "kind")?, None | Some("wasm")) {
        return Err(postcondition("registration is not stored WASM"));
    }
    let bytes = STANDARD
        .decode(required_string(row, "wasm_b64")?)
        .map_err(|_| malformed())?;
    let hash = ModuleHash::of(&bytes);
    let declared =
        ModuleHash::from_hex(required_string(row, "module_hash")?).map_err(|_| malformed())?;
    if declared != hash {
        return Err(postcondition(
            "stored registration module hash does not match its bytes",
        ));
    }
    let source = match row.get("source") {
        None | Some(QueryValue::Null) => None,
        Some(QueryValue::Str(source)) => Some(source.clone()),
        _ => return Err(malformed()),
    };
    let lang = required_string(row, "lang")?;
    if !matches!(
        (lang, source.as_ref()),
        ("wasm", None) | ("rust" | "shamir", Some(_))
    ) {
        return Err(malformed());
    }
    for field in ["owner", "group", "mode"] {
        match row.get(field) {
            None => {}
            Some(QueryValue::Null) if field == "group" => {}
            Some(QueryValue::Int(value))
                if *value >= 0 && (field != "mode" || u16::try_from(*value).is_ok()) => {}
            _ => return Err(malformed()),
        }
    }
    Ok(Registration {
        lang: lang.into(),
        source,
        bytes,
        hash,
        optimize: None,
        function_meta: None,
        stable_metadata: vec![ResourceMeta::from_record(row).to_query_value()],
    })
}

fn optional_u32(row: &QueryValue, field: &str) -> Result<QueryValue, BatchError> {
    match row.get(field) {
        None => Ok(QueryValue::Null),
        Some(QueryValue::Int(value)) if u32::try_from(*value).is_ok() => {
            Ok(QueryValue::Int(*value))
        }
        _ => Err(malformed()),
    }
}

fn tier_mode(value: &str) -> Option<TierMode> {
    match value {
        "auto" => Some(TierMode::Auto),
        "now" => Some(TierMode::Now),
        "never" => Some(TierMode::Never),
        _ => None,
    }
}

fn required_string<'a>(row: &'a QueryValue, field: &str) -> Result<&'a str, BatchError> {
    optional_string(row, field)?.ok_or_else(malformed)
}

fn optional_string<'a>(row: &'a QueryValue, field: &str) -> Result<Option<&'a str>, BatchError> {
    match row.get(field) {
        None => Ok(None),
        Some(QueryValue::Str(s)) => Ok(Some(s)),
        _ => Err(malformed()),
    }
}

fn text(value: Option<&str>) -> QueryValue {
    value.map_or(QueryValue::Null, |s| QueryValue::Str(s.into()))
}

fn strings(values: &[String]) -> QueryValue {
    QueryValue::List(values.iter().cloned().map(QueryValue::Str).collect())
}

fn backend() -> BatchError {
    coded(
        "schema_migration_durability_unavailable",
        "registration catalogue read failed".into(),
    )
}

fn postcondition(message: &str) -> BatchError {
    coded("schema_migration_postcondition_failed", message.into())
}

fn malformed() -> BatchError {
    postcondition("registration catalogue row is malformed or ambiguous")
}

fn incoherent() -> BatchError {
    postcondition("stored and live registrations disagree or are unavailable")
}

fn invalid_request() -> BatchError {
    postcondition("invalid registration specification")
}
