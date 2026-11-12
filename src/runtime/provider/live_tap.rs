//! Map provider frames to live items for watched agents only.
//!
//! Delta notifications are taken from the raw frame before lifecycle observation;
//! the shard attributes them by native thread. Lifecycle frames are copied when
//! the shard selects their owner, before routing, so a failing or draining
//! owner's viewer still sees them. Only an explicit set of item kinds is shown:
//! user-message echoes, hook prompts and unknown items never become live items.
mod projection;
use super::protocol::{IncomingFrame, ServerMethod};
use crate::runtime::live::{Event, Kind};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::VecDeque;

// Native method names of the streamed delta families Hive displays.
const AGENT_MESSAGE_DELTA: &str = "item/agentMessage/delta";
const COMMAND_OUTPUT_DELTA: &str = "item/commandExecution/outputDelta";
const REASONING_SUMMARY_DELTA: &str = "item/reasoning/summaryTextDelta";
// Raw reasoning stays opted out; a provider that sends it anyway is ignored here.
const RAW_REASONING_DELTA: &str = "item/reasoning/textDelta";
// Deltas held between shard passes; the oldest drop first when viewers fall behind.
const QUEUED_DELTAS: usize = 1024;
const QUEUED_BYTES: usize = 4 * 1024 * 1024;
// Threads remembered as having lost deltas before every viewer is resynced.
const QUEUED_GAPS: usize = 64;
const GAP_BYTES: usize = 64 * 1024;

/// One streamed delta, attributed to an agent by its native thread.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Delta {
    pub thread: String,
    pub event: Event,
}
impl Delta {
    fn bytes(&self) -> usize {
        [
            &self.event.turn_id,
            &self.event.item_id,
            &self.event.tool,
            &self.event.text,
            &self.event.status,
        ]
        .into_iter()
        .flatten()
        .fold(self.thread.capacity(), |total, value| {
            total.saturating_add(value.capacity())
        })
    }
}

/// Bounded deltas between the provider reader and the shard's publication.
///
/// Each delta keeps its provider read sequence, so the shard publishes it just
/// before the first later frame even when that frame was deferred during a call.
/// Overflow drops the oldest delta and records its thread, whose viewers then
/// resync instead of silently missing text.
#[derive(Default)]
pub(crate) struct Deltas {
    queue: VecDeque<(u64, Delta)>,
    gaps: std::collections::HashSet<String>,
    all_gaps: bool,
    bytes: usize,
    gap_bytes: usize,
}

/// Deltas ready for publication, after any threads that lost deltas.
pub(crate) struct Ready {
    pub deltas: Vec<Delta>,
    pub gaps: Vec<String>,
    pub all_gaps: bool,
}

impl Deltas {
    pub(crate) fn push(&mut self, seq: u64, delta: Delta) {
        let bytes = delta.bytes();
        if bytes > QUEUED_BYTES {
            self.gap(delta.thread);
            return;
        }
        while self.queue.len() >= QUEUED_DELTAS || self.bytes + bytes > QUEUED_BYTES {
            if let Some((_, lost)) = self.queue.pop_front() {
                self.bytes -= lost.bytes();
                self.gap(lost.thread);
            }
        }
        self.bytes += bytes;
        self.queue.push_back((seq, delta));
    }
    fn gap(&mut self, thread: String) {
        if self.all_gaps || self.gaps.contains(&thread) {
            return;
        }
        if self.gaps.len() >= QUEUED_GAPS
            || self.gap_bytes.saturating_add(thread.capacity()) > GAP_BYTES
        {
            self.all_gaps = true;
            self.gaps.clear();
            self.gap_bytes = 0;
        } else {
            self.gap_bytes += thread.capacity();
            self.gaps.insert(thread);
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.gaps.is_empty() && !self.all_gaps
    }
    /// Deltas read before `seq`, or all of them when no frame follows.
    pub(crate) fn before(&mut self, seq: Option<u64>) -> Ready {
        let count = seq.map_or(self.queue.len(), |seq| {
            self.queue
                .iter()
                .take_while(|(read, _)| *read < seq)
                .count()
        });
        let deltas: Vec<_> = self.queue.drain(..count).map(|(_, delta)| delta).collect();
        self.bytes -= deltas.iter().map(Delta::bytes).sum::<usize>();
        self.gap_bytes = 0;
        Ready {
            deltas,
            gaps: self.gaps.drain().collect(),
            all_gaps: std::mem::take(&mut self.all_gaps),
        }
    }
}

/// A delta notification: shown to viewers, or hidden raw reasoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeltaKind {
    Shown(Kind),
    Hidden,
}

/// The delta kind of a raw notification; `None` for every other frame.
pub(crate) fn delta_kind(frame: &Value) -> Option<DeltaKind> {
    match projection::Method::read(frame).method.as_deref()? {
        AGENT_MESSAGE_DELTA => Some(DeltaKind::Shown(Kind::Text)),
        COMMAND_OUTPUT_DELTA => Some(DeltaKind::Shown(Kind::ToolOutput)),
        REASONING_SUMMARY_DELTA => Some(DeltaKind::Shown(Kind::Reasoning)),
        RAW_REASONING_DELTA => Some(DeltaKind::Hidden),
        _ => None,
    }
}

/// A delta from a raw notification; malformed deltas are ignored, never fatal.
pub(crate) fn delta(kind: Kind, frame: &Value) -> Option<Delta> {
    let params = projection::Frame::read(frame).params;
    Some(Delta {
        thread: params.thread_id?.into_owned(),
        event: Event {
            turn_id: params.turn_id.map(Cow::into_owned),
            item_id: params.item_id.map(Cow::into_owned),
            text: Some(params.delta?.into_owned()),
            ..Event::new(kind)
        },
    })
}

/// The live copy of an allowlisted lifecycle frame.
pub(crate) fn event(frame: &IncomingFrame) -> Option<Event> {
    match frame.method()? {
        method @ (ServerMethod::TurnStarted | ServerMethod::TurnCompleted) => {
            let params = projection::Params::read(frame.params().raw());
            if !params.turn.present {
                return None;
            }
            let turn = params.turn.value;
            Some(Event {
                turn_id: turn.id.map(Cow::into_owned),
                status: turn.status.map(Cow::into_owned),
                ..Event::new(if *method == ServerMethod::TurnStarted {
                    Kind::TurnStarted
                } else {
                    Kind::TurnCompleted
                })
            })
        }
        method @ (ServerMethod::ItemStarted | ServerMethod::ItemCompleted) => {
            let params = projection::Params::read(frame.params().raw());
            let item = params.item;
            let (tool, summary) = item.tool()?;
            let started = *method == ServerMethod::ItemStarted;
            let exit = item.exit_code.map(|code| format!("exit {code}"));
            Some(Event {
                turn_id: params.turn_id.map(Cow::into_owned),
                item_id: item.id.map(Cow::into_owned),
                tool: Some(tool),
                text: if started { summary } else { exit },
                status: (!started)
                    .then(|| item.status.map(Cow::into_owned))
                    .flatten(),
                ..Event::new(if started {
                    Kind::ToolStarted
                } else {
                    Kind::ToolCompleted
                })
            })
        }
        _ => None,
    }
}

#[cfg(test)]
// Fixtures stop immediately on malformed literals.
mod tests;
