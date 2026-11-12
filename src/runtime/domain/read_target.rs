//! Classify read targets at input boundaries without validating their identities.
const ARTIFACT_PREFIX: &str = "artifact:";
const JOIN_PREFIX: &str = "join:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadTarget<'a> {
    Agent(&'a str),
    Artifact { raw: &'a str, id: &'a str },
    Join { raw: &'a str, id: &'a str },
}

impl<'a> ReadTarget<'a> {
    pub(crate) const JOIN_PREFIX_BYTES: usize = JOIN_PREFIX.len();

    pub(crate) fn parse(raw: &'a str) -> Self {
        if let Some(id) = raw.strip_prefix(ARTIFACT_PREFIX) {
            Self::Artifact { raw, id }
        } else if let Some(id) = raw.strip_prefix(JOIN_PREFIX) {
            Self::Join { raw, id }
        } else {
            Self::Agent(raw)
        }
    }

    pub(crate) fn as_str(self) -> &'a str {
        match self {
            Self::Agent(raw) | Self::Artifact { raw, .. } | Self::Join { raw, .. } => raw,
        }
    }

    pub(crate) fn is_agent(self) -> bool {
        matches!(self, Self::Agent(_))
    }

    pub(crate) fn artifact(self) -> Option<&'a str> {
        match self {
            Self::Artifact { id, .. } => Some(id),
            _ => None,
        }
    }
}

impl<'a, T: AsRef<str> + ?Sized> From<&'a T> for ReadTarget<'a> {
    fn from(raw: &'a T) -> Self {
        Self::parse(raw.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_preserves_raw_spelling_and_deferred_validation() {
        for raw in [
            "",
            "agent",
            "artifact:",
            "join:",
            "artifact:join:é",
            "join:artifact:é",
            "Artifact:ID",
            "join:é",
        ] {
            assert_eq!(ReadTarget::parse(raw).as_str(), raw);
        }
        assert_eq!(
            ReadTarget::parse("artifact:join:é").artifact(),
            Some("join:é")
        );
        assert_eq!(ReadTarget::parse("artifact:").artifact(), Some(""));
        assert!(ReadTarget::parse("Artifact:ID").is_agent());
        assert!(matches!(
            ReadTarget::parse("join:"),
            ReadTarget::Join { id: "", .. }
        ));
    }
}
