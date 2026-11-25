//! `ResetShapeStats` request DTO (R4b step 4).
//!
//! Pure wire/serde type: classification (local telemetry wipe) lives in
//! `BatchOp`, execution in the server's admin handler. The reset is a
//! SCRAM-session surface only — API-token (bearer-token) requests are denied
//! server-side, with no policy change to the typed surface.

use serde::de::{self, MapAccess};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `ResetShapeStats` request — wipes one repository's shape registry,
/// including the `(other)` fold row.
///
/// Not audited as data, only as an admin op.
///
/// Wire shape (flat tag per the admin-op convention): the
/// `reset_shape_stats` presence-only discriminator sits flat in the op map
/// next to `repo`. The discriminator is routing data only; it is written by
/// [`Serialize`] and consumed by [`Deserialize`] without being stored, so the
/// typed surface carries no marker field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetShapeStatsReq {
    /// Repository whose shape registry is wiped.
    pub repo: String,
}

impl Serialize for ResetShapeStatsReq {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("reset_shape_stats", &true)?;
        map.serialize_entry("repo", &self.repo)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for ResetShapeStatsReq {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReqVisitor;

        impl<'de> de::Visitor<'de> for ReqVisitor {
            type Value = ResetShapeStatsReq;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map carrying a reset_shape_stats request")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut repo = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "reset_shape_stats" => {
                            // Presence-only routing discriminator; value ignored.
                            let _: bool = map.next_value()?;
                        }
                        "repo" => repo = Some(map.next_value()?),
                        unknown => {
                            return Err(de::Error::unknown_field(
                                unknown,
                                &["reset_shape_stats", "repo"],
                            ));
                        }
                    }
                }
                Ok(ResetShapeStatsReq {
                    repo: repo.ok_or_else(|| de::Error::missing_field("repo"))?,
                })
            }
        }

        deserializer.deserialize_map(ReqVisitor)
    }
}
