//! Project only native launch fields without copying the provider's option map.
use crate::{decode::field, options::OptionMap};
use serde::{
    Deserialize, Deserializer,
    de::{Visitor, value::MapDeserializer},
};
use std::fmt;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Options {
    #[serde(rename = "claude.max_output_tokens", deserialize_with = "field")]
    pub output_tokens: Option<u64>,
    #[serde(rename = "claude.max_mcp_output_tokens", deserialize_with = "field")]
    pub mcp_tokens: Option<u64>,
    #[serde(rename = "claude.auto_compact_window", deserialize_with = "field")]
    pub compact_window: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub thinking: Thinking,
}
impl Options {
    pub(super) fn read(values: &OptionMap) -> Self {
        let entries = values.iter().map(|(name, value)| (name.as_str(), value));
        crate::decode::object(MapDeserializer::<_, serde_json::Error>::new(entries))
            .unwrap_or_default()
    }
}
#[derive(Default)]
pub(super) enum Thinking {
    #[default]
    Unset,
    Tokens(u64),
    On,
    Off,
}
impl Thinking {
    pub(super) fn tokens(&self) -> Option<u64> {
        match self {
            Self::Tokens(value) => Some(*value),
            _ => None,
        }
    }
    pub(super) fn enabled(&self) -> Option<bool> {
        match self {
            Self::Tokens(_) | Self::On => Some(true),
            Self::Off => Some(false),
            Self::Unset => None,
        }
    }
}
impl<'de> Deserialize<'de> for Thinking {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Mode;
        impl Visitor<'_> for Mode {
            type Value = Thinking;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a token count or thinking mode")
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Thinking, E> {
                Ok(Thinking::Tokens(value))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Thinking, E> {
                Ok(match value {
                    "on" => Thinking::On,
                    "off" => Thinking::Off,
                    _ => Thinking::Unset,
                })
            }
        }
        d.deserialize_any(Mode)
    }
}

#[cfg(test)]
mod tests;
