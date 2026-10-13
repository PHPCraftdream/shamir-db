use serde::de::{self, Deserialize, Deserializer};

/// Deserialize an optional enum while retaining the owning field name in errors.
pub(crate) fn deserialize_optional_field<'de, D, T>(
    deserializer: D,
    field: &'static str,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value = Option::<T>::deserialize(deserializer)
        .map_err(|error| de::Error::custom(format!("field `{field}`: {error}")))?;
    Ok(value)
}
