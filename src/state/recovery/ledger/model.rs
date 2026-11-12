//! Define strict persisted execution intent and ownership records.
use super::{LockIdentity, ProcessIdentity, Uuid};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(in crate::state::recovery) enum Class {
    Provider,
    Probe,
    CredentialWriter,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(in crate::state::recovery) enum ExecutionPhase {
    Intent,
    Authorized,
    Finished,
}
#[derive(Clone, Deserialize, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Group ownership, quiescence and confirmed cleanup are separate persisted evidence \
            facts."
)] // Independent durable proof and ownership facts.
#[serde(deny_unknown_fields)]
pub(in crate::state::recovery) struct Record {
    pub(crate) never_authorized: bool,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub(crate) conversation: Option<(String, String, String)>,
    pub(crate) revision: u64,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub(crate) account_identity: Option<String>,
    pub(crate) id: Uuid,
    pub(crate) generation: Uuid,
    pub(crate) run: Uuid,
    pub(crate) boot: String,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub(crate) owner: ProcessIdentity,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub(crate) lock: LockIdentity,
    pub(crate) class: Class,
    pub(crate) phase: ExecutionPhase,
    #[serde(deserialize_with = "hive_input::decode::optional_object")]
    pub(crate) process: Option<ProcessIdentity>,
    pub(crate) own_group: bool,
    pub(crate) quiescent: bool,
    pub(crate) confirmed: bool,
}
