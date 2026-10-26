use crate::value::Value;
use serde::de::{
    value::{MapAccessDeserializer, SeqAccessDeserializer},
    Deserialize, DeserializeSeed, MapAccess, SeqAccess,
};

#[test]
fn rv_sdk_e55bca90_huge_truncated_headers_do_not_preallocate() {
    for bytes in [
        &[0xdd, 0xff, 0xff, 0xff, 0xff][..],
        &[0xdf, 0xff, 0xff, 0xff, 0xff][..],
    ] {
        assert!(rmp_serde::from_slice::<Value>(bytes).is_err());
    }
}

struct HugeHintSeq;

impl<'de> SeqAccess<'de> for HugeHintSeq {
    type Error = serde::de::value::Error;

    fn next_element_seed<T>(&mut self, _seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        Ok(None)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(usize::MAX)
    }
}

struct HugeHintMap;

impl<'de> MapAccess<'de> for HugeHintMap {
    type Error = serde::de::value::Error;

    fn next_key_seed<K>(&mut self, _seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: DeserializeSeed<'de>,
    {
        Ok(None)
    }

    fn next_value_seed<V>(&mut self, _seed: V) -> Result<V::Value, Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        unreachable!("no map key was yielded")
    }

    fn size_hint(&self) -> Option<usize> {
        Some(usize::MAX)
    }
}

#[test]
fn rv_sdk_e55bca90_spoofed_size_hints_do_not_reserve() {
    let seq = SeqAccessDeserializer::new(HugeHintSeq);
    let map = MapAccessDeserializer::new(HugeHintMap);
    let list = Value::deserialize(seq).unwrap();
    let map = Value::deserialize(map).unwrap();
    assert_eq!(list, Value::List(Vec::new()));
    assert_eq!(map, Value::Map(Vec::new()));
    assert!(matches!(list, Value::List(values) if values.capacity() == 0));
    assert!(matches!(map, Value::Map(entries) if entries.capacity() == 0));
}
