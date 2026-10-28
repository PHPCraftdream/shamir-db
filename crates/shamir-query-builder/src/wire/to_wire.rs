use serde::Serialize;
use shamir_types::types::value::QueryValue;

use super::WireError;

pub trait ToWire: Serialize {
    fn to_query_value(&self) -> Result<QueryValue, WireError> {
        let bytes = rmp_serde::to_vec_named(self).map_err(WireError::Encode)?;
        rmp_serde::from_slice(&bytes).map_err(WireError::Decode)
    }

    fn to_msgpack(&self) -> Result<Vec<u8>, WireError> {
        rmp_serde::to_vec_named(self).map_err(WireError::Encode)
    }
}

impl<T: Serialize + ?Sized> ToWire for T {}
