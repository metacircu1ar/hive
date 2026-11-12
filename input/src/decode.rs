//! Exact named-record admission without losing duplicate fields to an intermediate map.
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    fmt,
    marker::PhantomData,
};
mod value;
pub use value::value;

struct Object<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Named<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Named<T> {
            type Value = Object<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a named record object")
            }
            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Object)
            }
        }
        decoder.deserialize_map(Named(PhantomData))
    }
}

pub fn object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Object::<T>::deserialize(d).map(|Object(v)| v)
}
pub fn optional_object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<Object<T>>::deserialize(d).map(|v| v.map(|Object(v)| v))
}
pub fn objects<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Vec::<Object<T>>::deserialize(d).map(|v| v.into_iter().map(|Object(v)| v).collect())
}

pub fn map<'de, D, K, V>(d: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Ord + Deserialize<'de>,
    V: Deserialize<'de>,
{
    struct Unique<K, V>(PhantomData<(K, V)>);
    impl<'de, K: Ord + Deserialize<'de>, V: Deserialize<'de>> Visitor<'de> for Unique<K, V> {
        type Value = BTreeMap<K, V>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map with unique decoded keys")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut input: M) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = input.next_entry()? {
                match result.entry(key) {
                    Entry::Vacant(entry) => {
                        entry.insert(value);
                    }
                    Entry::Occupied(_) => return Err(de::Error::custom("duplicate map key")),
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Unique(PhantomData))
}
pub fn object_map<'de, D, K, V>(d: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Ord + Deserialize<'de>,
    V: Deserialize<'de>,
{
    map::<D, K, Object<V>>(d).map(|v| v.into_iter().map(|(k, Object(v))| (k, v)).collect())
}
struct UniqueMap<K, V>(BTreeMap<K, V>);
impl<'de, K: Ord + Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de> for UniqueMap<K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        map(d).map(UniqueMap)
    }
}
pub fn optional_map<'de, D, K, V>(d: D) -> Result<Option<BTreeMap<K, V>>, D::Error>
where
    D: Deserializer<'de>,
    K: Ord + Deserialize<'de>,
    V: Deserialize<'de>,
{
    Option::<UniqueMap<K, V>>::deserialize(d).map(|v| v.map(|UniqueMap(v)| v))
}
/// A fixed sequence of nullable records; the sequence itself is part of the format.
pub fn optional_objects_array<'de, D, T, const N: usize>(d: D) -> Result<[Option<T>; N], D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let values = Vec::<Option<Object<T>>>::deserialize(d)?;
    let length = values.len();
    let values: [Option<Object<T>>; N] = values
        .try_into()
        .map_err(|_| de::Error::invalid_length(length, &"a fixed record sequence"))?;
    Ok(values.map(|v| v.map(|Object(v)| v)))
}
pub fn objects_array<'de, D, T, const N: usize>(d: D) -> Result<[T; N], D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let values = objects(d)?;
    let length = values.len();
    values
        .try_into()
        .map_err(|_| de::Error::invalid_length(length, &"a fixed record sequence"))
}
pub fn from_slice<T: de::DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = object(&mut decoder)?;
    decoder.end()?;
    Ok(result)
}
pub fn from_str<T: de::DeserializeOwned>(text: &str) -> serde_json::Result<T> {
    from_slice(text.as_bytes())
}
/// Arbitrary JSON values retain arrays and scalars, rejecting duplicate keys recursively.
pub fn json_value(bytes: &[u8]) -> serde_json::Result<serde_json::Value> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = value(&mut decoder)?;
    decoder.end()?;
    Ok(result)
}

#[cfg(test)]
mod tests;
