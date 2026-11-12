//! Borrowed quota fields. Invalid fields never erase valid sibling windows.
use super::{BALANCE_CHARS, CodexCredits};
use crate::runtime::provider::protocol::lenient;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::{borrow::Cow, collections::BTreeMap};

#[derive(Default)]
enum Limit<'a> {
    #[default]
    Unspecified,
    Named(&'a str),
    Malformed,
}
impl<'de: 'a, 'a> Deserialize<'de> for Limit<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Option::<&str>::deserialize(d) {
            Ok(None) => Self::Unspecified,
            Ok(Some(name)) => Self::Named(name),
            Err(_) => Self::Malformed,
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Codex<'a> {
    #[serde(default, borrow)]
    limit_id: Limit<'a>,
    #[serde(default, deserialize_with = "lenient::defaulting")]
    pub primary: ReportedWindow,
    #[serde(default, deserialize_with = "lenient::defaulting")]
    pub secondary: ReportedWindow,
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    pub credits: CreditMetadata<'a>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReportedWindow {
    #[serde(default, deserialize_with = "lenient::optional")]
    pub window_duration_mins: Option<u64>,
    #[serde(default, deserialize_with = "lenient::optional")]
    pub used_percent: Option<f64>,
    #[serde(default, deserialize_with = "lenient::optional")]
    pub utilization: Option<f64>,
    #[serde(default, deserialize_with = "lenient::optional")]
    pub resets_at: Option<u64>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreditMetadata<'a> {
    #[serde(default, deserialize_with = "lenient::optional")]
    has_credits: Option<bool>,
    #[serde(default, deserialize_with = "lenient::optional")]
    unlimited: Option<bool>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    balance: Option<Cow<'a, str>>,
}
impl CreditMetadata<'_> {
    pub(super) fn report(&self) -> Option<CodexCredits> {
        Some(CodexCredits {
            has_credits: self.has_credits?,
            unlimited: self.unlimited?,
            balance: self
                .balance
                .as_deref()
                .filter(|balance| {
                    (1..=BALANCE_CHARS).contains(&balance.len())
                        && balance
                            .chars()
                            .all(|c| c.is_ascii_digit() || ".,-".contains(c))
                })
                .map(str::to_owned),
        })
    }
}
impl<'a> Codex<'a> {
    pub(super) fn read(value: &'a Value) -> Self {
        lenient::defaulting(value).unwrap_or_default()
    }
    pub(super) fn account_limit(&self) -> bool {
        matches!(self.limit_id, Limit::Unspecified | Limit::Named("codex"))
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Claude<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    pub unified_windows: BTreeMap<&'a str, WindowEntry>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub overage_status: Option<Cow<'a, str>>,
    #[serde(default, deserialize_with = "lenient::optional")]
    pub is_using_overage: Option<bool>,
}
#[derive(Deserialize)]
pub(super) struct WindowEntry(
    #[serde(deserialize_with = "lenient::defaulting")] pub ReportedWindow,
);
impl<'a> Claude<'a> {
    pub(super) fn read(value: &'a Value) -> Self {
        lenient::defaulting(value).unwrap_or_default()
    }
}
