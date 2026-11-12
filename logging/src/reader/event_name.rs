//! Shared event-filter grammar; human adapters retain their own error wording.
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventName(String);
impl EventName {
    pub fn parse(text: &str) -> Result<Self, InvalidEventName> {
        if !text.is_empty()
            && text.len() <= 128
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        {
            Ok(Self(text.into()))
        } else {
            Err(InvalidEventName)
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Debug)]
pub struct InvalidEventName;
impl fmt::Display for InvalidEventName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Expected an event name of 1..128 ASCII letters, digits, _, ., : or -")
    }
}
impl std::error::Error for InvalidEventName {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_grammar_keeps_exact_bytes_and_bounds() -> Result<(), InvalidEventName> {
        for text in ["a", "agent.completed", "_:A-9", &"x".repeat(128)] {
            let name = EventName::parse(text)?;
            assert_eq!(name.as_str(), text);
            assert_eq!(name.into_string(), text);
        }
        for text in ["", "with space", "é", "line\n", &"x".repeat(129)] {
            assert!(EventName::parse(text).is_err());
        }
        Ok(())
    }
}
