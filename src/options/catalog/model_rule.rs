//! Catalog selection rules; capability names remain catalog-defined identifiers.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug)]
pub enum ModelRule {
    Any,
    LiveCatalog,
    LiveEfforts,
    ClaudeThinking,
    Capability(String),
}

impl From<String> for ModelRule {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "any" => Self::Any,
            "live_catalog" => Self::LiveCatalog,
            "live_efforts" => Self::LiveEfforts,
            "claude_thinking" => Self::ClaudeThinking,
            _ => Self::Capability(raw),
        }
    }
}
impl ModelRule {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Any => "any",
            Self::LiveCatalog => "live_catalog",
            Self::LiveEfforts => "live_efforts",
            Self::ClaudeThinking => "claude_thinking",
            Self::Capability(name) => name,
        }
    }
}
impl Serialize for ModelRule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for ModelRule {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_names_preserve_dynamic_capabilities_and_string_wire() -> Result<(), serde_json::Error> {
        for raw in [
            "any",
            "live_catalog",
            "live_efforts",
            "claude_thinking",
            "future_rule",
            "",
        ] {
            let encoded = serde_json::to_string(raw)?;
            let rule: ModelRule = serde_json::from_str(&encoded)?;
            assert_eq!(rule.as_str(), raw);
            assert_eq!(serde_json::to_string(&rule)?, encoded);
        }
        assert!(matches!(
            ModelRule::from("future_rule".to_owned()),
            ModelRule::Capability(_)
        ));
        Ok(())
    }
}
