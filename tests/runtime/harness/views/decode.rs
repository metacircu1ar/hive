//! Wire checks keep typed fixture observations as strict as the original JSON assertions.
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub(super) fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub(super) fn string_enum<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let text = String::deserialize(deserializer)?;
    T::deserialize(serde::de::value::StringDeserializer::<D::Error>::new(text))
}

pub(super) fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    object_value(value).map_err(serde::de::Error::custom)
}

fn object_value<T: DeserializeOwned>(value: Value) -> serde_json::Result<T> {
    if !value.is_object() {
        return Err(serde::de::Error::custom("expected an object"));
    }
    serde_json::from_value(value)
}

pub(super) fn objects<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Vec::<Value>::deserialize(deserializer)?
        .into_iter()
        .map(object_value)
        .collect::<serde_json::Result<Vec<T>>>()
        .map_err(serde::de::Error::custom)
}

pub(super) fn exact_object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Serialize,
{
    let value = Value::deserialize(deserializer)?;
    exact_value(value).map_err(serde::de::Error::custom)
}

fn exact_value<T: DeserializeOwned + Serialize>(value: Value) -> serde_json::Result<T> {
    let decoded = object_value(value.clone())?;
    if serde_json::to_value(&decoded)? != value {
        return Err(serde::de::Error::custom(
            "typed observation would normalize its wire shape",
        ));
    }
    Ok(decoded)
}

pub(super) fn nullable_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    object_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

pub(super) fn nullable_exact_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Serialize,
{
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    exact_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

pub(super) fn present_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    object(deserializer).map(Some)
}
