//! Parse part names once, retaining invalid spellings for deferred validation.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Hive,
    Core,
    Comb,
    /// Comb's bundled static `comb-llama-server`, a separately frozen part.
    CombLlama,
    Unknown(String),
}
impl From<String> for Part {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "hive" => Self::Hive,
            "hive-core" => Self::Core,
            "comb" => Self::Comb,
            "comb-llama" => Self::CombLlama,
            _ => Self::Unknown(raw),
        }
    }
}
impl From<&str> for Part {
    fn from(raw: &str) -> Self {
        raw.to_owned().into()
    }
}
impl Part {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Hive => "hive",
            Self::Core => "hive-core",
            Self::Comb => "comb",
            Self::CombLlama => "comb-llama",
            Self::Unknown(raw) => raw,
        }
    }
}
impl AsRef<Path> for Part {
    fn as_ref(&self) -> &Path {
        Path::new(self.as_str())
    }
}
impl fmt::Display for Part {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
impl Serialize for Part {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for Part {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_names_keep_string_bytes_and_deferred_rejection() -> crate::Result<()> {
        for raw in [
            "hive",
            "hive-core",
            "comb",
            "comb-llama",
            "unknown",
            "",
            "é",
        ] {
            let encoded = serde_json::to_string(raw)?;
            let part: Part = serde_json::from_str(&encoded)?;
            assert_eq!(part.as_str(), raw);
            assert_eq!(serde_json::to_string(&part)?, encoded);
        }
        let Err(actual) = serde_json::from_str::<Part>("true") else {
            panic!("part accepted a non-string");
        };
        let Err(original) = serde_json::from_str::<String>("true") else {
            panic!("string accepted a boolean");
        };
        assert_eq!(actual.to_string(), original.to_string());
        assert!(matches!(Part::from("unknown"), Part::Unknown(_)));
        assert_eq!(Part::from("comb-llama"), Part::CombLlama);
        Ok(())
    }
}
