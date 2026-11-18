//! `ListActivity` request DTO and its selector enums (R4b step 1, §5.1).
//!
//! Pure wire/serde types: classification (read-only, server-owned state
//! introspection) lives in `BatchOp`, execution in the server's admin
//! handler. `All` sees every user's rows (needs the server-side
//! `ActivityView` permission); `Own` is open to any authenticated user and
//! is filtered server-side.
//!
//! Surface restriction: the operation is a SCRAM-session surface only —
//! API-token (bearer-token) requests are denied server-side
//! (`token_request_denied`, `shamir-server-security` `http_rest/scope.rs`),
//! with no policy change to the typed surface.

use serde::de::{self, MapAccess};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Scope of an activity listing.
///
/// Scope widens what the server renders; it never widens who may call:
/// the operation itself is denied for API-token requests (SCRAM sessions
/// only) before the scope is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityScope {
    /// Every user's rows (requires server-side `ActivityView` + `Manage`).
    All,
    /// Only the calling user's rows (any authenticated user).
    Own,
}

/// Which registry the listing renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Sessions,
    Requests,
    Transactions,
    Cursors,
    /// All four registries in one page.
    All,
}

/// `ListActivity` request — one bounded page of the activity view.
///
/// Wire shape (flat tag per the admin-op convention): the
/// `list_activity` presence-only discriminator sits flat in the op map
/// next to the fields below. The discriminator is routing data only; it
/// is written by [`Serialize`] and consumed by [`Deserialize`] without
/// being stored, so the typed surface carries no marker field.
#[derive(Debug, Clone, PartialEq)]
pub struct ListActivityReq {
    /// Which users' rows to render.
    pub scope: ActivityScope,
    /// Which registry to render.
    pub kind: ActivityKind,
    /// Requested page size; server applies its own global cap (default 256).
    pub limit: Option<u32>,
    /// Opaque continuation token from a previous [`crate::admin::activity::ActivityPage`].
    pub continuation: Option<String>,
}

impl Serialize for ListActivityReq {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let field_count =
            2 + usize::from(self.limit.is_some()) + usize::from(self.continuation.is_some());
        let mut map = serializer.serialize_map(Some(field_count + 1))?;
        map.serialize_entry("list_activity", &true)?;
        map.serialize_entry("scope", &self.scope)?;
        map.serialize_entry("kind", &self.kind)?;
        if let Some(limit) = &self.limit {
            map.serialize_entry("limit", limit)?;
        }
        if let Some(continuation) = &self.continuation {
            map.serialize_entry("continuation", continuation)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ListActivityReq {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReqVisitor;

        impl<'de> de::Visitor<'de> for ReqVisitor {
            type Value = ListActivityReq;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map carrying a list_activity request")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ListActivityReq, A::Error> {
                let mut scope = None;
                let mut kind = None;
                let mut limit = None;
                let mut continuation = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "list_activity" => {
                            // Presence-only routing discriminator; value ignored.
                            let _: bool = map.next_value()?;
                        }
                        "scope" => scope = Some(map.next_value()?),
                        "kind" => kind = Some(map.next_value()?),
                        "limit" => limit = Some(map.next_value()?),
                        "continuation" => continuation = Some(map.next_value()?),
                        unknown => {
                            return Err(de::Error::unknown_field(
                                unknown,
                                &["list_activity", "scope", "kind", "limit", "continuation"],
                            ));
                        }
                    }
                }
                Ok(ListActivityReq {
                    scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
                    kind: kind.ok_or_else(|| de::Error::missing_field("kind"))?,
                    limit,
                    continuation,
                })
            }
        }

        deserializer.deserialize_map(ReqVisitor)
    }
}
