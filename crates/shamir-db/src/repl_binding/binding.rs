//! [`SubscriptionBinding`] — the durable destination set of a subscription
//! row (S5c-X PR8b-2). Canonical: destinations are deduplicated and sorted
//! so equality comparison is deterministic.

use crate::query::admin::ReplStream;
use crate::types::value::QueryValue;
use shamir_query_types::admin::ReplDirection;
use shamir_storage::error::DbError;

/// Version of the on-row binding format. Rows recording any other version
/// are refused with a typed catalogue error (no released format to migrate).
pub const SUBSCRIPTION_BINDING_VERSION: u64 = 1;

/// The executable destination set recorded on a subscription row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionBinding {
    version: u64,
    generation: u64,
    /// Canonical (deduplicated, sorted) `(db, repo)` Pull destinations.
    destinations: Vec<(String, String)>,
}

/// Validate one destination pair; `subscription` names the row in errors.
fn validate_destination(subscription: &str, db: &str, repo: &str) -> Result<(), DbError> {
    if db.is_empty() || repo.is_empty() {
        return Err(DbError::subscription_catalogue(
            subscription,
            format!("destination '{db}/{repo}' has an empty db or repo name"),
        ));
    }
    if db == "*" || repo == "*" {
        // Database-wide Pull scopes are rejected at binding time — the
        // destination set must be concrete (reviewer verdict §1).
        return Err(DbError::subscription_catalogue(
            subscription,
            format!("destination '{db}/{repo}' uses a wildcard; only concrete Pull destinations can be bound"),
        ));
    }
    Ok(())
}

/// Canonicalize + validate a raw destination list.
fn canonicalize(
    subscription: &str,
    mut destinations: Vec<(String, String)>,
) -> Result<Vec<(String, String)>, DbError> {
    if destinations.is_empty() {
        return Err(DbError::subscription_catalogue(
            subscription,
            "binding has an empty destination set",
        ));
    }
    for (db, repo) in &destinations {
        validate_destination(subscription, db, repo)?;
    }
    destinations.sort();
    destinations.dedup();
    Ok(destinations)
}

impl SubscriptionBinding {
    /// Build a binding for a row writer; runs the same canonicalization and
    /// validation as [`Self::from_row`].
    pub fn new(
        version: u64,
        generation: u64,
        destinations: Vec<(String, String)>,
    ) -> Result<Self, DbError> {
        if version != SUBSCRIPTION_BINDING_VERSION {
            return Err(DbError::subscription_catalogue(
                "<unnamed>",
                format!("unsupported binding version {version}"),
            ));
        }
        if generation < 1 {
            return Err(DbError::subscription_catalogue(
                "<unnamed>",
                "binding_generation must be >= 1".to_string(),
            ));
        }
        Ok(Self {
            version,
            generation,
            destinations: canonicalize("<unnamed>", destinations)?,
        })
    }

    /// Strict parse of a subscription row's binding fields. Every failure is
    /// a typed [`DbError::subscription_catalogue`] naming the row's `name`
    /// (fallback `"<unnamed>"`).
    pub fn from_row(row: &QueryValue) -> Result<Self, DbError> {
        let name = row
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("<unnamed>");
        let fail = |reason: String| DbError::subscription_catalogue(name, reason);

        let version = row
            .get("binding_version")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| fail("missing or non-integer 'binding_version'".to_string()))?;
        if version != SUBSCRIPTION_BINDING_VERSION {
            return Err(fail(format!("unsupported binding version {version}")));
        }
        let generation = row
            .get("binding_generation")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| fail("missing or non-integer 'binding_generation'".to_string()))?;
        if generation < 1 {
            return Err(fail("binding_generation must be >= 1".to_string()));
        }
        let entries = row
            .get("binding_destinations")
            .and_then(|value| value.as_array())
            .ok_or_else(|| fail("missing or non-list 'binding_destinations'".to_string()))?;
        let mut destinations = Vec::with_capacity(entries.len());
        for entry in entries {
            let db = entry
                .get("db")
                .and_then(|value| value.as_str())
                .ok_or_else(|| fail("destination entry missing 'db' string".to_string()))?;
            let repo = entry
                .get("repo")
                .and_then(|value| value.as_str())
                .ok_or_else(|| fail("destination entry missing 'repo' string".to_string()))?;
            validate_destination(name, db, repo)?;
            let pair = (db.to_string(), repo.to_string());
            if destinations.contains(&pair) {
                return Err(fail(format!(
                    "ambiguous duplicate destination '{}/{}'",
                    pair.0, pair.1
                )));
            }
            destinations.push(pair);
        }
        Ok(Self {
            version,
            generation,
            destinations: canonicalize(name, destinations)?,
        })
    }

    /// The executable destination set for a subscription: Pull-direction
    /// streams only, each with an explicit concrete `repo` (a database-wide
    /// Pull scope is rejected here) and no table scope. Sorted +
    /// deduplicated. An empty result is an error.
    pub fn from_streams(
        subscription: &str,
        streams: &[ReplStream],
    ) -> Result<Vec<(String, String)>, DbError> {
        let mut destinations = Vec::new();
        for stream in streams {
            if !matches!(stream.direction, ReplDirection::Pull) {
                continue;
            }
            if stream.scope.table.is_some() {
                return Err(DbError::subscription_catalogue(
                    subscription,
                    format!(
                        "table-scoped Pull stream over '{}/…' cannot be bound",
                        stream.scope.db
                    ),
                ));
            }
            let repo = stream.scope.repo.as_deref().unwrap_or("");
            validate_destination(subscription, &stream.scope.db, repo)?;
            destinations.push((stream.scope.db.clone(), repo.to_string()));
        }
        destinations.sort();
        destinations.dedup();
        if destinations.is_empty() {
            return Err(DbError::subscription_catalogue(
                subscription,
                "profile has no concrete Pull destination".to_string(),
            ));
        }
        Ok(destinations)
    }

    /// Insert the binding fields into a subscription-row map.
    pub fn write_to_row(&self, row: &mut QueryValue) {
        let destinations: Vec<QueryValue> = self
            .destinations
            .iter()
            .map(|(db, repo)| {
                let mut fields = crate::types::common::new_map();
                fields.insert("db".to_string(), QueryValue::Str(db.clone()));
                fields.insert("repo".to_string(), QueryValue::Str(repo.clone()));
                QueryValue::Map(fields)
            })
            .collect();
        if let QueryValue::Map(fields) = row {
            fields.insert(
                "binding_version".to_string(),
                QueryValue::Int(self.version as i64),
            );
            fields.insert(
                "binding_generation".to_string(),
                QueryValue::Int(self.generation as i64),
            );
            fields.insert(
                "binding_destinations".to_string(),
                QueryValue::List(destinations),
            );
        }
    }

    /// The canonical destination set.
    pub fn destinations(&self) -> &[(String, String)] {
        &self.destinations
    }

    /// Durable binding generation — changed whenever the executable binding
    /// changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// On-row binding format version.
    pub fn version(&self) -> u64 {
        self.version
    }
}
