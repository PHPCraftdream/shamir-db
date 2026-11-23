//! `ListShapeStats` request DTO (R4b step 4).
//!
//! Pure wire/serde type: classification (read-only introspection of the
//! node-local shape registry) lives in `BatchOp`, execution in the server's
//! admin handler. The listing is a SCRAM-session surface only — API-token
//! (bearer-token) requests are denied server-side, with no policy change to
//! the typed surface.

use serde::de::{self, MapAccess};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `ListShapeStats` request — one bounded page of per-shape statistics for
/// ONE repository.
///
/// `limit` defaults to 50 server-side and is clamped to the registry cap
/// (256). The page is ordered by `total_nanos` desc, then `executions` desc,
/// then `shape_hex` asc.
///
/// Wire shape (flat tag per the admin-op convention): the
/// `list_shape_stats` presence-only discriminator sits flat in the op map
/// next to the fields below. The discriminator is routing data only; it is
/// written by [`Serialize`] and consumed by [`Deserialize`] without being
/// stored, so the typed surface carries no marker field.
#[derive(Debug, Clone, PartialEq)]
pub struct ListShapeStatsReq {
    /// Repository whose shape registry is rendered.
    pub repo: String,
    /// Requested page size; `None` means the server default (50), and any
    /// value is clamped to the registry cap (256).
    pub limit: Option<u32>,
}

impl Serialize for ListShapeStatsReq {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let field_count = 1 + usize::from(self.limit.is_some());
        let mut map = serializer.serialize_map(Some(field_count + 1))?;
        map.serialize_entry("list_shape_stats", &true)?;
        map.serialize_entry("repo", &self.repo)?;
        if let Some(limit) = &self.limit {
            map.serialize_entry("limit", limit)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ListShapeStatsReq {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReqVisitor;

        impl<'de> de::Visitor<'de> for ReqVisitor {
            type Value = ListShapeStatsReq;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map carrying a list_shape_stats request")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut repo = None;
                let mut limit = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "list_shape_stats" => {
                            // Presence-only routing discriminator; value ignored.
                            let _: bool = map.next_value()?;
                        }
                        "repo" => repo = Some(map.next_value()?),
                        "limit" => limit = Some(map.next_value()?),
                        unknown => {
                            return Err(de::Error::unknown_field(
                                unknown,
                                &["list_shape_stats", "repo", "limit"],
                            ));
                        }
                    }
                }
                Ok(ListShapeStatsReq {
                    repo: repo.ok_or_else(|| de::Error::missing_field("repo"))?,
                    limit,
                })
            }
        }

        deserializer.deserialize_map(ReqVisitor)
    }
}
