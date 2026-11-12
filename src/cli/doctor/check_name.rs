//! Project diagnostic names for setup decisions while preserving display names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::cli) enum CheckName<'a> {
    Daemon,
    Services,
    Comb,
    Auth(&'a str),
    Other,
}
impl<'a> CheckName<'a> {
    pub(in crate::cli) fn parse(raw: &'a str) -> Self {
        match raw {
            "daemon" => Self::Daemon,
            "services" => Self::Services,
            _ if raw.starts_with("comb:") => Self::Comb,
            _ => raw.strip_prefix("auth:").map_or(Self::Other, Self::Auth),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_names_keep_prefix_and_unknown_fallback_semantics() {
        assert!(matches!(CheckName::parse("comb:"), CheckName::Comb));
        assert!(matches!(CheckName::parse("comb:role:a"), CheckName::Comb));
        assert!(matches!(
            CheckName::parse("auth:a:b"),
            CheckName::Auth("a:b")
        ));
        assert!(matches!(CheckName::parse("Comb:a"), CheckName::Other));
        assert!(matches!(CheckName::parse("daemon"), CheckName::Daemon));
    }
}
