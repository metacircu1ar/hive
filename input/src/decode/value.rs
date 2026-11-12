//! Recursive JSON value decoding that refuses duplicate keys before collection.
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::fmt;

struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(Json)
    }
}
struct Json;
impl<'de> Visitor<'de> for Json {
    type Value = Unique;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON with unique object keys")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut input: M) -> Result<Unique, M::Error> {
        let mut result = Map::new();
        while let Some((key, Unique(value))) = input.next_entry::<String, Unique>()? {
            if result.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate object key"));
            }
        }
        Ok(Unique(Value::Object(result)))
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut input: S) -> Result<Unique, S::Error> {
        let result = std::iter::from_fn(|| input.next_element::<Unique>().transpose())
            .map(|entry| entry.map(|Unique(value)| value))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Unique(Value::Array(result)))
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Unique, E> {
        Ok(Unique(Value::Bool(v)))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Unique, E> {
        Ok(Unique(Value::from(v)))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Unique, E> {
        Ok(Unique(Value::from(v)))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Unique, E> {
        serde_json::Number::from_f64(v)
            .map(|v| Unique(Value::Number(v)))
            .ok_or_else(|| de::Error::custom("non-finite JSON number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Unique, E> {
        Ok(Unique(Value::String(v.to_owned())))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Unique, E> {
        Ok(Unique(Value::String(v)))
    }
}
pub fn value<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    Unique::deserialize(d).map(|Unique(v)| v)
}
