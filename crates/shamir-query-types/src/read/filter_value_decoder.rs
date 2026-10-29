//! Deterministic decoding of filter values and marker maps.

use std::fmt;

use serde::de::{Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::FilterValue;

impl<'de> Deserialize<'de> for FilterValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(FilterValueVisitor)
    }
}

struct FilterValueVisitor;

impl<'de> Visitor<'de> for FilterValueVisitor {
    type Value = FilterValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a filter value")
    }

    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(FilterValue::Null)
    }

    // The previous untagged derive accepted both nil callbacks as `Null`.
    fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
        Ok(FilterValue::Null)
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(FilterValue::Bool(value))
    }

    fn visit_i64<E: Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(FilterValue::Int(value))
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(match i64::try_from(value) {
            Ok(value) => FilterValue::Int(value),
            Err(_) => FilterValue::String(value.to_string()),
        })
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Self::Value, E> {
        Ok(FilterValue::Float(value))
    }

    fn visit_f32<E: Error>(self, value: f32) -> Result<Self::Value, E> {
        Ok(FilterValue::Float(f64::from(value)))
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(FilterValue::String(value.to_owned()))
    }

    fn visit_string<E: Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(FilterValue::String(value))
    }

    fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        Ok(FilterValue::Binary(value.to_vec()))
    }

    fn visit_byte_buf<E: Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        Ok(FilterValue::Binary(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element()? {
            values.push(value);
        }
        Ok(FilterValue::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut marker: Option<(&str, FilterValue)> = None;
        let mut path: Option<Option<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == "path" {
                if path.is_some() {
                    return Err(A::Error::custom("duplicate key path"));
                }
                path = Some(
                    map.next_value()
                        .map_err(|e| A::Error::custom(format!("invalid path: {e}")))?,
                );
                continue;
            }
            let name = match key.as_str() {
                "$ref" => "$ref",
                "$query" => "$query",
                "$fn" => "$fn",
                "$expr" => "$expr",
                "$cond" => "$cond",
                "$param" => "$param",
                _ => return Err(A::Error::custom(format!("unexpected key {key}"))),
            };
            if let Some((previous, _)) = &marker {
                return Err(A::Error::custom(format!(
                    "conflicting keys {previous} and {name}"
                )));
            }
            let value = match name {
                "$ref" => map.next_value().map(|path| FilterValue::FieldRef { path }),
                "$query" => map
                    .next_value()
                    .map(|alias| FilterValue::QueryRef { alias, path: None }),
                "$fn" => map.next_value().map(|call| FilterValue::FnCall { call }),
                "$expr" => map.next_value().map(|expr| FilterValue::Expr { expr }),
                "$cond" => map.next_value().map(|cond| FilterValue::Cond { cond }),
                "$param" => map.next_value().map(|name| FilterValue::Param { name }),
                _ => return Err(A::Error::custom("invalid discriminator")),
            }
            .map_err(|e| A::Error::custom(format!("invalid {name}: {e}")))?;
            marker = Some((name, value));
        }
        match (marker, path) {
            (Some((_, FilterValue::QueryRef { alias, .. })), path) => Ok(FilterValue::QueryRef {
                alias,
                path: path.flatten(),
            }),
            (Some((name, _)), Some(_)) => {
                Err(A::Error::custom(format!("path not allowed with {name}")))
            }
            (Some((_, value)), None) => Ok(value),
            (None, Some(_)) => Err(A::Error::custom("path requires $query")),
            (None, None) => Err(A::Error::custom("missing filter value discriminator")),
        }
    }
}
