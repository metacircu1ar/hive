//! Bounded native authentication metadata decoding.
use super::{Deserialize, Error, ObservedMethod, Operation, Report, Result, Status, process};
use crate::enums::wire_enum;
mod codex;
pub(super) use codex::parse_codex;
use std::str;

pub(super) fn verify_pending(operation: &Operation, report: &Report) -> Result<()> {
    match report.state {
        Status::Authenticated => operation.confirm_account(report.account_identity.clone()),
        Status::Missing | Status::ReauthenticationRequired => operation.confirm_missing(),
        Status::MethodConflict => Err(Error::policy_conflict(concat!(
            "The selected authentication method conflicts with native credentials. ",
            "Correct provider configuration or authenticate an isolated replacement.",
        ))),
        _ => Err(report.diagnostic.clone().unwrap_or_else(|| {
            Error::recovery_required(concat!(
                "Login verification is incomplete. Retry auth recover or auth login after ",
                "the provider becomes available; the original credential namespace and ",
                "evidence were retained.",
            ))
        })),
    }
}

wire_enum!(ClaudeAuthMethod {
    ClaudeAi => "claude.ai",
    None => "none",
    ApiKeySnake => "api_key",
    ApiKey => "apiKey",
    OauthToken => "oauth_token",
    ApiKeyHelper => "apiKeyHelper",
    Bedrock => "bedrock",
    Vertex => "vertex",
});
wire_enum!(ClaudeApiProvider {
    FirstParty => "firstParty",
    Bedrock => "bedrock",
    Vertex => "vertex",
    Foundry => "foundry",
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeStatus {
    logged_in: bool,
    auth_method: ClaudeAuthMethod,
    api_provider: ClaudeApiProvider,
    email: Option<String>,
}
pub(super) fn parse_claude(exit: Option<i32>, bytes: &[u8], report: &mut Report) -> Result<()> {
    let status: ClaudeStatus =
        serde_json::from_slice(bytes).map_err(|cause| process::malformed().with_source(cause))?;
    if !matches!(exit, Some(libc::EXIT_SUCCESS | libc::EXIT_FAILURE)) {
        return Err(Error::provider_failed("Provider status command failed."));
    }
    report.credential_present = Some(status.logged_in);
    match (status.logged_in, status.auth_method, status.api_provider) {
        (true, ClaudeAuthMethod::ClaudeAi, ClaudeApiProvider::FirstParty)
            if exit == Some(libc::EXIT_SUCCESS) =>
        {
            report.state = Status::Authenticated;
            report.observed_method = Some(ObservedMethod::Subscription);
            report.account_identity = status.email;
        }
        (false, ClaudeAuthMethod::None, ClaudeApiProvider::FirstParty) => {
            report.state = Status::Missing;
        }
        (false, ClaudeAuthMethod::ClaudeAi, ClaudeApiProvider::FirstParty) => {
            report.state = Status::ReauthenticationRequired;
        }
        (
            _,
            ClaudeAuthMethod::ApiKeySnake
            | ClaudeAuthMethod::ApiKey
            | ClaudeAuthMethod::OauthToken
            | ClaudeAuthMethod::ApiKeyHelper
            | ClaudeAuthMethod::Bedrock
            | ClaudeAuthMethod::Vertex,
            _,
        )
        | (
            _,
            _,
            ClaudeApiProvider::Bedrock | ClaudeApiProvider::Vertex | ClaudeApiProvider::Foundry,
        ) => {
            report.state = Status::MethodConflict;
            report.observed_method = Some(ObservedMethod::Other);
        }
        _ => return Err(process::malformed()),
    }
    Ok(())
}
