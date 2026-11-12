//! Preserve positive root evidence instead of treating a missing field as null.
use serde::{Deserialize, Deserializer};

#[derive(Default)]
pub(crate) enum Parent {
    #[default]
    Unspecified,
    Root,
    Child(String),
}

impl<'de> Deserialize<'de> for Parent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)? {
            Some(id) => Self::Child(id),
            None => Self::Root,
        })
    }
}

impl Parent {
    pub(crate) fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Child(id) => Some(id),
            Self::Unspecified | Self::Root => None,
        }
    }
    pub(crate) fn is_some(&self) -> bool {
        matches!(self, Self::Child(_))
    }
    pub(crate) fn is_root(&self) -> bool {
        matches!(self, Self::Root)
    }
    pub(crate) fn into_option(self) -> Option<String> {
        match self {
            Self::Child(id) => Some(id),
            Self::Unspecified | Self::Root => None,
        }
    }
}
