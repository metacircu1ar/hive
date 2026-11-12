//! Named JSON records and literal enums for `Hive`'s own stored data.
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, Visitor},
};
use std::{borrow::Cow, fmt, marker::PhantomData};

/// Borrow retained Value strings and plain input; own escaped decoder text.
pub(crate) fn text<'de, D: Deserializer<'de>>(d: D) -> Result<Cow<'de, str>, D::Error> {
    struct Text;
    impl<'de> Visitor<'de> for Text {
        type Value = Cow<'de, str>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string")
        }
        fn visit_borrowed_str<E: serde::de::Error>(
            self,
            value: &'de str,
        ) -> Result<Self::Value, E> {
            Ok(Cow::Borrowed(value))
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Ok(Cow::Owned(value.to_owned()))
        }
        fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
            Ok(Cow::Owned(value))
        }
    }
    d.deserialize_str(Text)
}

pub(crate) use hive_input::decode::{object, optional_object};

pub(crate) fn string_enum<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    struct Literal<T>(PhantomData<T>);
    impl<T: DeserializeOwned> Visitor<'_> for Literal<T> {
        type Value = T;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a literal enum string")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<T, E> {
            T::deserialize(serde::de::value::StrDeserializer::new(value))
        }
    }
    deserializer.deserialize_str(Literal(PhantomData))
}

pub(crate) fn from_str<T: DeserializeOwned>(text: &str) -> serde_json::Result<T> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = object(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

// Lenient optional-field projections operate on already parsed JSON.
pub(crate) fn field<'de, D, T>(input: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    T::deserialize(input).or_else(|_| Ok(T::default()))
}

pub(crate) fn object_or_default<'de, D, T>(input: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    crate::decode::object(input).or_else(|_| Ok(T::default()))
}
