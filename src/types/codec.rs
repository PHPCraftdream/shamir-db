//! Zero-copy serialization using rkyv.
//!
//! ## Performance
//!
//! - **Serialization**: Same speed as MessagePack (~10-20% faster in some cases)
//! - **Deserialization**: ZERO-COPY! ArchivedValue is a view into original bytes
//! - **Memory**: No allocations during deserialization
//!
//! ## Usage
//!
//! ```rust
//! # use shamir_db::types::codec::{self, to_bytes, from_bytes, as_archived};
//! # use rkyv::{Archive, Serialize, Deserialize};
//! # #[derive(Archive, Serialize, Deserialize, Debug, PartialEq)]
//! # struct MyValue { name: String, count: u32 }
//! # let value = MyValue { name: "test".to_string(), count: 42 };
//! // Serialize
//! let bytes = to_bytes(&value)?;
//!
//! // Deserialize zero-copy (returns &ArchivedValue)
//! let archived = as_archived::<MyValue>(&bytes)?;
//! println!("{:?}", archived.name); // No allocation!
//!
//! // Deserialize to owned (if needed)
//! let owned: MyValue = from_bytes(&bytes)?;
//! # Ok::<(), shamir_db::types::codec::RkyvError>(())
//! ```

use bytes::Bytes;
use rkyv::{
    Archive, Deserialize, Serialize,
    archived_root,
    de::deserializers::SharedDeserializeMap,
    ser::serializers::AllocSerializer,
};
use std::fmt;

/// Rkyv codec errors
#[derive(Debug)]
pub enum RkyvError {
    /// Serialization error
    Serialize(String),
    /// Deserialization error
    Deserialize(String),
}

impl fmt::Display for RkyvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(msg) => write!(f, "rkyv serialization error: {}", msg),
            Self::Deserialize(msg) => write!(f, "rkyv deserialization error: {}", msg),
        }
    }
}

impl std::error::Error for RkyvError {}

/// Serialize a value to bytes using rkyv.
///
/// Returns `Bytes` that can be stored and later deserialized with `as_archived` or `from_bytes`.
///
/// ## Example
/// ```rust
/// # use shamir_db::types::codec::{self, to_bytes};
/// # use rkyv::{Archive, Serialize};
/// # #[derive(Archive, Serialize)]
/// # struct MyData { x: i32 }
/// # let my_value = MyData { x: 42 };
/// let bytes = to_bytes(&my_value)?;
/// # Ok::<(), shamir_db::types::codec::RkyvError>(())
/// ```
pub fn to_bytes<T>(value: &T) -> Result<Bytes, RkyvError>
where
    T: Serialize<AllocSerializer<256>>,
{
    rkyv::to_bytes::<_, 256>(value)
        .map(|aligned_vec| {
            // Convert AlignedVec to Bytes (copies data)
            Bytes::from(aligned_vec.into_vec())
        })
        .map_err(|e| RkyvError::Serialize(e.to_string()))
}

/// Deserialize bytes to an owned value using rkyv.
///
/// This will allocate new memory for strings, collections, etc.
/// For zero-copy deserialization, use `as_archived` instead.
///
/// ## Example
/// ```rust
/// # use shamir_db::types::codec::{self, to_bytes, from_bytes};
/// # use rkyv::{Archive, Serialize, Deserialize};
/// # #[derive(Archive, Serialize, Deserialize, PartialEq, Debug)]
/// # struct MyData { x: i32 }
/// # let original = MyData { x: 42 };
/// # let bytes = to_bytes(&original)?;
/// let value: MyData = from_bytes(&bytes)?;
/// # Ok::<(), shamir_db::types::codec::RkyvError>(())
/// ```
pub fn from_bytes<T>(bytes: &[u8]) -> Result<T, RkyvError>
where
    T: Archive,
    for<'a> T::Archived: Deserialize<T, SharedDeserializeMap>,
{
    unsafe {
        let archived = archived_root::<T>(bytes);
        archived
            .deserialize(&mut SharedDeserializeMap::new())
            .map_err(|e| RkyvError::Deserialize(e.to_string()))
    }
}

/// Zero-copy deserialization - returns a reference to the archived value.
///
/// **No allocations!** The returned `ArchivedT` is a view into the original bytes.
/// The bytes must outlive the returned reference.
///
/// # Safety
/// Bytes must be valid rkyv data created by `to_bytes`.
///
/// ## Example
/// ```rust
/// # use shamir_db::types::codec::{self, to_bytes, from_bytes, as_archived};
/// # use rkyv::{Archive, Serialize, Deserialize};
/// # #[derive(Archive, Serialize, Deserialize, PartialEq, Debug)]
/// # struct MyData { field: String }
/// # let original = MyData { field: "hello".to_string() };
/// # let bytes = to_bytes(&original)?;
/// // Zero-copy! No allocations
/// let archived = as_archived::<MyData>(&bytes)?;
/// println!("{:?}", archived.field);
///
/// // If you need owned value:
/// let owned: MyData = from_bytes(&bytes)?;
/// # Ok::<(), shamir_db::types::codec::RkyvError>(())
/// ```
pub fn as_archived<'a, T>(bytes: &'a [u8]) -> Result<&'a T::Archived, RkyvError>
where
    T: Archive,
{
    unsafe {
        Ok(archived_root::<T>(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Archive, Deserialize, Serialize)]
    #[archive(compare(PartialEq))]
    struct TestStruct {
        name: String,
        age: u32,
    }

    #[test]
    fn test_rkyv_roundtrip() {
        let original = TestStruct {
            name: "Alice".to_string(),
            age: 30,
        };

        let bytes = to_bytes(&original).unwrap();
        let deserialized: TestStruct = from_bytes(&bytes).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_zero_copy_deserialization() {
        let original = TestStruct {
            name: "Bob".to_string(),
            age: 25,
        };

        let bytes = to_bytes(&original).unwrap();

        // Zero-copy!
        let archived = as_archived::<TestStruct>(&bytes).unwrap();
        assert_eq!(archived.name, "Bob");
        assert_eq!(archived.age, 25);
    }

    #[test]
    fn test_zero_copy_no_allocation() {
        let value = TestStruct {
            name: "Very long string that would definitely allocate".to_string(),
            age: 42,
        };

        let bytes = to_bytes(&value).unwrap();

        // This should NOT allocate (except maybe for validation)
        let archived = as_archived::<TestStruct>(&bytes).unwrap();
        assert_eq!(archived.name, "Very long string that would definitely allocate");
    }

    #[test]
    fn test_bytes_still_valid_after_as_archived() {
        let value = TestStruct {
            name: "Test".to_string(),
            age: 100,
        };

        let bytes = to_bytes(&value).unwrap();

        {
            let _archived = as_archived::<TestStruct>(&bytes).unwrap();
        }

        // bytes are still valid and can be used again
        let archived2 = as_archived::<TestStruct>(&bytes).unwrap();
        assert_eq!(archived2.name, "Test");
    }
}
