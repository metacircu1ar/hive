//! Preserve native account-probe presence and partial report updates.
use super::{Error, ObservedMethod, Report, Result, Status, process};
use crate::decode::field;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

crate::enums::wire_enum!(AuthAccountKind {
    Chatgpt => "chatgpt",
    ApiKey => "apiKey",
    AmazonBedrock => "amazonBedrock",
});

#[derive(Default, Deserialize)]
#[serde(default)]
struct Probe<'a> {
    #[serde(borrow)]
    result: ResultField<'a>,
}
#[derive(Default)]
enum ResultField<'a> {
    #[default]
    Missing,
    Present(AccountResult<'a>),
}
impl<'de: 'a, 'a> Deserialize<'de> for ResultField<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        crate::decode::object_or_default(d).map(Self::Present)
    }
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AccountResult<'a> {
    #[serde(deserialize_with = "field")]
    requires_openai_auth: Option<bool>,
    #[serde(borrow)]
    account: Account<'a>,
}
#[derive(Default)]
enum Account<'a> {
    #[default]
    Missing,
    Null,
    Data(Metadata<'a>),
    Invalid,
}
impl<'de: 'a, 'a> Deserialize<'de> for Account<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        crate::decode::optional_object(d)
            .map(|value| value.map_or(Self::Null, Self::Data))
            .or(Ok(Self::Invalid))
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Metadata<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "field")]
    kind: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    email: Option<&'a str>,
}

pub(in crate::auth) fn parse_codex(value: &Value, report: &mut Report) -> Result<()> {
    let probe: Probe<'_> = crate::decode::object(value).unwrap_or_default();
    let ResultField::Present(result) = probe.result else {
        return Err(Error::provider_protocol_error(
            "Account probe failed; provider details were withheld.",
        ));
    };
    if !result.requires_openai_auth.ok_or_else(process::malformed)? {
        report.state = Status::MethodConflict;
        report.diagnostic = Some(Error::policy_conflict(
            "Provider configuration does not select the required OpenAI authentication.",
        ));
        return Ok(());
    }
    account(result.account, report)
}

fn account(account: Account<'_>, report: &mut Report) -> Result<()> {
    match account {
        Account::Missing => return Err(process::malformed()),
        Account::Null => {
            report.state = Status::Missing;
            report.credential_present = Some(false);
        }
        present => {
            report.credential_present = Some(true);
            let Account::Data(metadata) = present else {
                return Err(process::malformed());
            };
            match metadata.kind.map(AuthAccountKind::parse) {
                Some(AuthAccountKind::Chatgpt) => {
                    report.state = Status::Authenticated;
                    report.observed_method = Some(ObservedMethod::Subscription);
                    report.account_identity = metadata.email.map(str::to_owned);
                }
                Some(AuthAccountKind::ApiKey | AuthAccountKind::AmazonBedrock) => {
                    report.state = Status::MethodConflict;
                    report.observed_method = Some(ObservedMethod::Other);
                }
                _ => return Err(process::malformed()),
            }
        }
    }
    Ok(())
}
