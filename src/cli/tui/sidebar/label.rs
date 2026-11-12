//! Known domain values drive badges; unknown spellings retain their display text.
use super::{AgentState, TaskOutcome};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::cli::tui) struct Label<T> {
    raw: String,
    value: Option<T>,
}
impl<T> Label<T> {
    fn new(raw: String, parse: fn(&str) -> Option<T>) -> Self {
        let value = parse(&raw);
        Self { raw, value }
    }
}
impl<T: Copy> Label<T> {
    pub(super) fn known(&self) -> Option<T> {
        self.value
    }
}
impl From<String> for Label<AgentState> {
    fn from(raw: String) -> Self {
        Self::new(raw, AgentState::parse)
    }
}
impl From<String> for Label<TaskOutcome> {
    fn from(raw: String) -> Self {
        Self::new(raw, TaskOutcome::parse)
    }
}
impl<T> fmt::Display for Label<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.raw)
    }
}
