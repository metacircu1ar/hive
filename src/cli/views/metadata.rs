//! Approval cards, quota summaries and command-failure presentation fields.
use super::{field, object, present_object, rows};
use crate::error::ErrorCode;
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct ErrorReply<'a> {
    #[serde(borrow, deserialize_with = "object")]
    pub error: Failure<'a>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Failure<'a> {
    #[serde(deserialize_with = "field")]
    pub code: Option<ErrorCode>,
    #[serde(borrow, deserialize_with = "field")]
    pub message: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub field: Option<&'a str>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Approval<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub state: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub title: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub message: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub destructive: bool,
    pub schema: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct UsageReport {
    #[serde(deserialize_with = "field")]
    pub auth: Option<String>,
    #[serde(deserialize_with = "field")]
    pub generation: Option<String>,
    #[serde(deserialize_with = "field")]
    pub observed_at: Option<u64>,
    #[serde(deserialize_with = "rows")]
    pub windows: Vec<Window>,
    #[serde(deserialize_with = "present_object")]
    pub credits: Option<Credits>,
    #[serde(deserialize_with = "present_object")]
    pub extra_usage: Option<ExtraUsage>,
    #[serde(deserialize_with = "field")]
    pub native_session: Option<crate::runtime::domain::NativeSessionUsage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Window {
    #[serde(deserialize_with = "field")]
    pub name: Option<String>,
    #[serde(deserialize_with = "field")]
    pub used_percent: f64,
    #[serde(deserialize_with = "field")]
    pub minutes: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub resets_at: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Credits {
    #[serde(deserialize_with = "field")]
    pub unlimited: bool,
    #[serde(rename = "has_credits", deserialize_with = "field")]
    pub available: bool,
    #[serde(deserialize_with = "field")]
    pub balance: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct ExtraUsage {
    #[serde(deserialize_with = "field")]
    pub in_use: bool,
    #[serde(deserialize_with = "field")]
    pub status: Option<String>,
}
