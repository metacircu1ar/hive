//! Bounded native executable version text.
use crate::{error::Result, process, provider::Provider};
use std::str;

// Maximum provider version text accepted for an authentication diagnostic.
const VERSION_BYTES: usize = 40;

pub(crate) fn parse_version(provider: Provider, output: &[u8]) -> Result<String> {
    let text = str::from_utf8(output)
        .map_err(|cause| process::malformed().with_source(cause))?
        .trim();
    let version = match provider {
        Provider::Codex => text.strip_prefix("codex-cli "),
        Provider::Claude => text.strip_suffix(" (Claude Code)"),
    }
    .ok_or_else(process::malformed)?;
    if version.is_empty()
        || version.len() > VERSION_BYTES
        || !version.bytes().next().is_some_and(|c| c.is_ascii_digit())
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        return Err(process::malformed());
    }
    Ok(version.into())
}
