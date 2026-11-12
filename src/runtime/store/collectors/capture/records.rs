//! Canonical collector records, serialized only at publication boundaries.
use crate::{config::CollectionSource, runtime::collectors::Evidence};
use serde::Serialize;

#[derive(Serialize)]
#[serde(untagged)]
pub(in crate::runtime::store) enum Automatic<'a> {
    Agent {
        source: CollectionSource,
        actor: &'a str,
        role: Option<&'a str>,
        execution: Option<&'a str>,
        turn: Option<&'a str>,
        evidence: Outcome<'a>,
    },
    Approval {
        source: CollectionSource,
        actor: &'static str,
        subject_agent: &'a str,
        subject_role: Option<&'a str>,
        execution: Option<&'a str>,
        turn: Option<&'a str>,
        approval_id: Option<&'a str>,
        evidence: Approval,
    },
}

#[derive(Serialize)]
pub(in crate::runtime::store) struct Outcome<'a> {
    pub outcome: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<crate::error::ErrorCode>,
}
#[derive(Serialize)]
pub(in crate::runtime::store) struct Approval {
    pub action: crate::approvals::ApprovalAction,
}
#[derive(Serialize)]
pub(super) struct Submission<'a> {
    pub source: CollectionSource,
    pub actor: &'a str,
    pub role: Option<&'a str>,
    pub execution: Option<&'a str>,
    pub turn: Option<&'a str>,
    pub evidence: &'a Evidence,
}
#[derive(Serialize)]
pub(super) struct Overflow<'a> {
    pub destination: &'a str,
    pub first_skipped_event: i64,
}
#[derive(Serialize)]
pub(super) struct Envelope<'a, T> {
    pub format: u32,
    pub installation: &'a str,
    pub event: i64,
    pub record: &'a T,
}
#[derive(Serialize)]
pub(super) struct Gap<'a> {
    pub format: u32,
    pub installation: &'a str,
    pub kind: &'static str,
    pub destination: &'a str,
    pub skipped_count: i64,
    pub first_event: Option<i64>,
    pub last_event: Option<i64>,
}
