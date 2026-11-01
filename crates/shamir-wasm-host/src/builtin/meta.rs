//! Function catalogue metadata types (slice 9).
//!
//! [`Visibility`] and [`Security`] are stored in the catalogue but NOT
//! enforced at the facade — enforcement belongs to the wire layer (slice 10).
//! [`FunctionMeta`] is the in-memory metadata kept alongside the live
//! function registry.
//!
//! [`CreateFunctionOptions`] is the options bag accepted by
//! `ShamirDb::create_function_with_opts`.

use shamir_types::types::value::QueryValue;

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
    /// Host patterns this function may reach via `ctx.http_fetch()`,
    /// INTERSECTED with the DB-wide `net_allowlist` (a function can never
    /// exceed the DB's own ceiling — see `build_net_gateway`).
    ///
    /// Task #609: matches `secret_grants`'s restrictive-by-default
    /// precedent — an EMPTY/absent `net_grants` means NO egress for this
    /// function. A non-empty grant list can only ask for LESS than the
    /// DB-wide `net_allowlist`, never more (see `build_net_gateway`'s doc
    /// comment for the full reasoning). Builtin functions with no
    /// catalogue row are a separate code path and keep the full
    /// allowlist — this restrictive default applies to user-created
    /// functions going through `CreateFunctionOptions`.
    pub net_grants: Vec<String>,
}

impl FunctionMeta {
    /// Construct metadata with the given fields.
    pub fn new(
        visibility: Visibility,
        security: Security,
        secret_grants: Vec<String>,
        net_grants: Vec<String>,
    ) -> Self {
        Self {
            visibility,
            security,
            secret_grants,
            net_grants,
        }
    }

    /// Construct metadata from a persisted `QueryValue` catalogue record.
    ///
    /// Lossy legacy compatibility decoder: malformed or non-string values silently fall back to
    /// defaults (Private visibility, Invoker security, and empty grant lists).
    /// Use [`FunctionMeta::try_from_record`] for checked persisted-data decoding.
    #[deprecated(note = "lossy legacy decoder; use FunctionMeta::try_from_record")]
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
        let net_grants = rec
            .get("net_grants")
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
            net_grants,
        }
    }

    /// Decode a catalogue record, rejecting malformed metadata.
    /// Unversioned legacy records default only absent fields. Version 1 requires
    /// all four fields written by `inject_into`; unsupported versions and
    /// malformed present fields are rejected.
    pub fn try_from_record(rec: &shamir_types::types::value::QueryValue) -> Result<Self, String> {
        let QueryValue::Map(map) = rec else {
            return Err("function metadata record must be a map".into());
        };
        match map.get("metadata_version") {
            None => {}
            Some(QueryValue::Int(1)) => {
                for field in ["visibility", "security", "secret_grants", "net_grants"] {
                    if !map.contains_key(field) {
                        return Err(format!("missing {field} in metadata version 1"));
                    }
                }
            }
            Some(_) => return Err("unsupported or malformed metadata_version".into()),
        }
        let visibility = match map.get("visibility") {
            None => Visibility::Private,
            Some(QueryValue::Str(value)) => value.parse().map_err(|_| "invalid visibility")?,
            Some(_) => return Err("invalid visibility".into()),
        };
        let security = match map.get("security") {
            None => Security::Invoker,
            Some(QueryValue::Str(value)) => value.parse().map_err(|_| "invalid security")?,
            Some(_) => return Err("invalid security".into()),
        };
        let grants = |name: &str| -> Result<Vec<String>, String> {
            match map.get(name) {
                None => Ok(Vec::new()),
                Some(QueryValue::List(values)) => values
                    .iter()
                    .map(|value| match value {
                        QueryValue::Str(value) => Ok(value.clone()),
                        _ => Err(format!("invalid {name} entry")),
                    })
                    .collect(),
                Some(_) => Err(format!("invalid {name}")),
            }
        };
        Ok(Self {
            visibility,
            security,
            secret_grants: grants("secret_grants")?,
            net_grants: grants("net_grants")?,
        })
    }

    /// Inject metadata fields into a `QueryValue::Map` record for persistence.
    pub fn inject_into(&self, rec: &mut shamir_types::types::value::QueryValue) {
        if let shamir_types::types::value::QueryValue::Map(map) = rec {
            map.insert(
                "metadata_version".to_string(),
                shamir_types::types::value::QueryValue::Int(1),
            );
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
            // Build the net_grants list as QueryValue::List.
            let net_grants: Vec<shamir_types::types::value::QueryValue> = self
                .net_grants
                .iter()
                .map(|s| shamir_types::types::value::QueryValue::Str(s.clone()))
                .collect();
            map.insert(
                "net_grants".to_string(),
                shamir_types::types::value::QueryValue::List(net_grants),
            );
        }
    }
}

/// Options bag for [`crate::FunctionRegistry`] creation via the
/// facade's `create_function_with_opts`.
///
/// Default matches pre-slice-9 behaviour: replace=false, Private, Invoker,
/// no secret grants. For user-created functions an EMPTY/absent `net_grants`
/// means NO egress for this function (fail-closed, like `secret_grants`);
/// granting egress requires explicitly listing host patterns, which are then
/// intersected with the DB-wide `net_allowlist` (a function can never exceed
/// the DB ceiling). Builtin functions with no catalogue row are a separate
/// code path and keep the full allowlist. See [`FunctionMeta::net_grants`]
/// and `build_net_gateway`.
#[derive(Debug, Clone)]
pub struct CreateFunctionOptions {
    pub replace: bool,
    pub visibility: Visibility,
    pub security: Security,
    pub secret_grants: Vec<String>,
    pub net_grants: Vec<String>,
}

impl Default for CreateFunctionOptions {
    fn default() -> Self {
        Self {
            replace: false,
            visibility: Visibility::Private,
            security: Security::Invoker,
            secret_grants: Vec::new(),
            net_grants: Vec::new(),
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
