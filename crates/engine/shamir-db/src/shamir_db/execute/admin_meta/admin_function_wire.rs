//! Admin handlers: DescribeFunction, ExportFunction, CheckFunction, AlterFunction.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::shamir_db::FunctionSource;
use crate::shamir_db::FunctionState;
use crate::types::value::QueryValue;
use crate::DbError;
use shamir_engine::function::{Tier, TierMode, UpgradeState};
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

/// Facade authorization failures arrive as `DbError::Function("access denied: ...")`;
/// give them the structured `access_denied` code like the other admin handlers.
fn facade_err(e: DbError) -> BatchError {
    let code = match &e {
        DbError::Function(m) if m.starts_with("access denied:") => {
            Some("access_denied".to_string())
        }
        _ => None,
    };
    let message = match (&code, e) {
        (Some(_), DbError::Function(m)) => m,
        (_, e) => e.to_string(),
    };
    BatchError::QueryError {
        alias: String::new(),
        message,
        code,
    }
}

/// Stable wire name of a tier mode.
fn optimize_name(mode: TierMode) -> &'static str {
    match mode {
        TierMode::Auto => "auto",
        TierMode::Now => "now",
        TierMode::Never => "never",
    }
}

/// Inverse of [`optimize_name`]; `None` for an unknown string.
pub(super) fn parse_optimize(s: &str) -> Option<TierMode> {
    match s {
        "auto" => Some(TierMode::Auto),
        "now" => Some(TierMode::Now),
        "never" => Some(TierMode::Never),
        _ => None,
    }
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Fast => "fast",
        Tier::Optimized => "optimized",
    }
}

fn upgrade_state_name(state: UpgradeState) -> &'static str {
    match state {
        UpgradeState::Idle => "idle",
        UpgradeState::Scheduled => "scheduled",
        UpgradeState::Failed => "failed",
        UpgradeState::Done => "done",
        UpgradeState::Exhausted => "exhausted",
    }
}

/// `lang` validation shared by create/check (PROC-0 D6 rule 1): the explicit
/// `lang` must name the supplied payload field. `payload` is `Some("wasm")`,
/// `Some("source")` or `None`; callers only invoke this against a supplied
/// field, so `None` never reaches the mismatch arms. `lang = "shamir"` names
/// the source field — compiled in-process by the host shamir service.
pub(super) fn validate_lang(lang: Option<&str>, payload: Option<&str>) -> Result<(), String> {
    match lang {
        Some("shamir") if payload == Some("wasm") => Err(
            "lang \"shamir\" requires the \"source\" field, but \"wasm\" was supplied".to_string(),
        ),
        Some("wasm") if payload == Some("source") => Err(
            "lang \"wasm\" requires the \"wasm\" field, but \"source\" was supplied".to_string(),
        ),
        Some("rust") if payload == Some("wasm") => Err(
            "lang \"rust\" requires the \"source\" field, but \"wasm\" was supplied".to_string(),
        ),
        Some(v) if v != "wasm" && v != "rust" && v != "shamir" => Err(format!(
            "unknown lang \"{v}\" (must be \"wasm\", \"rust\" or \"shamir\")"
        )),
        _ => Ok(()),
    }
}

fn opt_str(v: Option<String>) -> QueryValue {
    match v {
        Some(s) => QueryValue::Str(s),
        None => QueryValue::Null,
    }
}

fn opt_int(v: Option<i64>) -> QueryValue {
    match v {
        Some(i) => QueryValue::Int(i),
        None => QueryValue::Null,
    }
}

impl ShamirAdminExecutor {
    pub(super) async fn handle_describe_function(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::DescribeFunction(op) = batch_op else {
            unreachable!("handle_describe_function called with non-DescribeFunction op");
        };

        let d = self
            .shamir
            .describe_function_as(&op.describe_function, self.actor.clone())
            .await
            .map_err(facade_err)?;

        let tier_qv = match &d.tier_report {
            Some(t) => mpack!({
                "tier": @(QueryValue::Str(tier_name(t.tier).to_string())),
                "mode": @(QueryValue::Str(optimize_name(t.mode).to_string())),
                "calls_observed": @(QueryValue::Int(t.calls_observed as i64)),
                "upgrade_state": @(QueryValue::Str(upgrade_state_name(t.upgrade_state).to_string())),
            }),
            None => QueryValue::Null,
        };
        let state_qv = match &d.state {
            FunctionState::Loaded => mpack!({
                "kind": "loaded",
            }),
            FunctionState::Unloadable { reason } => mpack!({
                "kind": "unloadable",
                "reason": @(QueryValue::Str(reason.clone())),
            }),
            FunctionState::Corrupt { expected, actual } => mpack!({
                "kind": "corrupt",
                "expected": @(QueryValue::Str(expected.clone())),
                "actual": @(QueryValue::Str(actual.clone())),
            }),
        };

        Ok(admin_result(mpack!({
            "name": @(QueryValue::Str(d.name.clone())),
            "lang": @(QueryValue::Str(d.lang.clone())),
            "origin": @opt_str(d.origin_claim.clone()),
            "abi": @opt_int(d.abi_version.map(|v| v as i64)),
            "hash": @opt_str(d.hash.clone()),
            "size": @opt_int(d.byte_size.map(|v| v as i64)),
            "optimize": @(QueryValue::Str(optimize_name(d.optimize).to_string())),
            "tier": @tier_qv,
            "state": @state_qv,
        })))
    }

    pub(super) async fn handle_export_function(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::ExportFunction(op) = batch_op else {
            unreachable!("handle_export_function called with non-ExportFunction op");
        };

        let e = self
            .shamir
            .export_function_as(&op.export_function, self.actor.clone())
            .await
            .map_err(facade_err)?;

        Ok(admin_result(mpack!({
            "wasm": @(QueryValue::Str(STANDARD.encode(&e.bytes))),
            "source": @opt_str(e.source),
            "hash": @(QueryValue::Str(e.hash)),
        })))
    }

    pub(super) async fn handle_check_function(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::CheckFunction(op) = batch_op else {
            unreachable!("handle_check_function called with non-CheckFunction op");
        };
        let spec = &op.check_function;

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };

        if spec.wasm.is_some() == spec.source.is_some() {
            return Err(err(
                "check_function requires exactly one of 'source' or 'wasm'".to_string(),
            ));
        }
        let payload = if spec.wasm.is_some() {
            "wasm"
        } else {
            "source"
        };
        validate_lang(spec.lang.as_deref(), Some(payload)).map_err(err)?;

        let optimize = match spec.optimize.as_deref() {
            Some(v) => match parse_optimize(v) {
                Some(m) => m,
                None => {
                    return Err(err(format!(
                        "unknown optimize \"{v}\" (must be \"auto\", \"now\" or \"never\")"
                    )));
                }
            },
            None => TierMode::Auto,
        };

        let report = if let Some(ref wasm_b64) = spec.wasm {
            let bytes = STANDARD
                .decode(wasm_b64)
                .map_err(|e| err(format!("invalid base64 wasm: {e}")))?;
            self.shamir
                .check_function_as(self.actor.clone(), FunctionSource::Wasm(&bytes), optimize)
                .await
                .map_err(facade_err)?
        } else {
            // `spec.source` is Some: the exactly-one guard above proves it.
            let src = spec.source.as_deref().expect("source present");
            // Lang-aware dispatch: shamir compiles in-process through the
            // check-only service path.
            let shamir = spec.lang.as_deref() == Some("shamir");
            let source = if shamir {
                FunctionSource::Shamir(src)
            } else {
                FunctionSource::Source(src)
            };
            self.shamir
                .check_function_as(self.actor.clone(), source, optimize)
                .await
                .map_err(facade_err)?
        };

        Ok(admin_result(mpack!({
            "ok": true,
            "size": @(QueryValue::Int(report.byte_size as i64)),
            "abi": @(QueryValue::Int(report.abi_version as i64)),
            "origin": @opt_str(report.origin_claim),
            "hash": @(QueryValue::Str(report.hash.to_hex())),
            "functions": @(QueryValue::Int(report.function_count as i64)),
            "imports": @(QueryValue::Int(report.import_count as i64)),
            "exports": @(QueryValue::Int(report.export_count as i64)),
        })))
    }

    pub(super) async fn handle_alter_function(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::AlterFunction(op) = batch_op else {
            unreachable!("handle_alter_function called with non-AlterFunction op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };

        let mode = match parse_optimize(&op.optimize) {
            Some(m) => m,
            None => {
                return Err(err(format!(
                    "unknown optimize \"{}\" (must be \"auto\", \"now\" or \"never\")",
                    op.optimize
                )));
            }
        };

        self.shamir
            .alter_function_optimize_as(&op.alter_function, mode, self.actor.clone())
            .await
            .map_err(facade_err)?;

        Ok(admin_result(mpack!({
            "altered_function": @(QueryValue::Str(op.alter_function.clone())),
            "optimize": @(QueryValue::Str(optimize_name(mode).to_string())),
        })))
    }
}
