//! Read targets share one route while retaining their distinct typed payloads.
use super::{EventRecord, EventsPage, HistoryPage, OutputResponse};
use crate::runtime::api::joins::JoinSnapshot;
use serde::Serialize;

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum ReadOutput {
    Events(EventsPage<Vec<EventRecord>>),
    History(HistoryPage<Vec<EventRecord>>),
    Output(OutputResponse),
    Join(JoinSnapshot),
}
