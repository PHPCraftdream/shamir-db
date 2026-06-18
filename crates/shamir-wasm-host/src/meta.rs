//! Function catalogue metadata types (slice 9).
//!
//! [`Visibility`] and [`Security`] are stored in the catalogue but NOT
//! enforced at the facade — enforcement belongs to the wire layer (slice 10).
//! [`FunctionMeta`] is the in-memory metadata kept alongside the live
//! function registry.
//!
//! [`CreateFunctionOptions`] is the options bag accepted by
//! `ShamirDb::create_function_with_opts`.

/// Who may see that the function exists.
///
/// Stored in the catalogue; enforcement is deferred to slice 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Visibility::Public => write!(f, "public"),
            Visibility::Private => write!(f, "private"),
        }
    }
}

impl std::str::FromStr for Visibility {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "public" => Ok(Visibility::Public),
            "private" => Ok(Visibility::Private),
            _ => Err(format!("unknown visibility: {s}")),
        }
    }
}

/// Whose privileges the function executes with.
///
/// Stored in the catalogue; enforcement is deferred to slice 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Invoker,
    Definer,
}

impl std::fmt::Display for Security {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Security::Invoker => write!(f, "invoker"),
            Security::Definer => write!(f, "definer"),
        }
    }
}

impl std::str::FromStr for Security {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "invoker" => Ok(Security::Invoker),
            "definer" => Ok(Security::Definer),
            _ => Err(format!("unknown security: {s}")),
        }
    }
}

/// In-memory function metadata stored alongside the live registry.
///
/// Populated on create/load, updated on rename, removed on drop.
#[derive(Debug, Clone)]
pub struct FunctionMeta {
    pub visibility: Visibility,
    pub security: Security,
    pub secret_grants: Vec<String>,
}

impl FunctionMeta {
    /// Construct metadata with the given fields.
    pub fn new(visibility: Visibility, security: Security, secret_grants: Vec<String>) -> Self {
        Self {
            visibility,
            security,
            secret_grants,
        }
    }

    /// Construct metadata from a persisted `QueryValue` catalogue record.
    ///
    /// Missing fields fall back to defaults (Private / Invoker / empty).
    pub fn from_record(rec: &shamir_types::types::value::QueryValue) -> Self {
        let visibility = rec
            .get("visibility")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(Visibility::Private);
        let security = rec
            .get("security")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(Security::Invoker);
        let secret_grants = rec
            .get("secret_grants")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            visibility,
            security,
            secret_grants,
        }
    }

    /// Inject metadata fields into a `QueryValue::Map` record for persistence.
    pub fn inject_into(&self, rec: &mut shamir_types::types::value::QueryValue) {
        if let shamir_types::types::value::QueryValue::Map(map) = rec {
            map.insert(
                "visibility".to_string(),
                shamir_types::types::value::QueryValue::Str(self.visibility.to_string()),
            );
            map.insert(
                "security".to_string(),
                shamir_types::types::value::QueryValue::Str(self.security.to_string()),
            );
            // Build the secret_grants list as QueryValue::List.
            let grants: Vec<shamir_types::types::value::QueryValue> = self
                .secret_grants
                .iter()
                .map(|s| shamir_types::types::value::QueryValue::Str(s.clone()))
                .collect();
            map.insert(
                "secret_grants".to_string(),
                shamir_types::types::value::QueryValue::List(grants),
            );
        }
    }
}

/// Options bag for [`crate::FunctionRegistry`] creation via the
/// facade's `create_function_with_opts`.
///
/// Default matches pre-slice-9 behaviour: replace=false, Private, Invoker,
/// no secret grants.
#[derive(Debug, Clone)]
pub struct CreateFunctionOptions {
    pub replace: bool,
    pub visibility: Visibility,
    pub security: Security,
    pub secret_grants: Vec<String>,
}

impl Default for CreateFunctionOptions {
    fn default() -> Self {
        Self {
            replace: false,
            visibility: Visibility::Private,
            security: Security::Invoker,
            secret_grants: Vec::new(),
        }
    }
}

impl CreateFunctionOptions {
    /// Construct with `replace = true`, keeping other fields at default.
    pub fn replace() -> Self {
        Self {
            replace: true,
            ..Self::default()
        }
    }
}
