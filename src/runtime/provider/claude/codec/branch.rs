//! Hold native side effects and output until this fork's actual history verifies.
use super::{Codec, Result, Value, protocol};
use crate::runtime::{
    branching::history::Snapshot,
    provider::queue::{ByteQueue, Limits},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(25);
const HELD: Limits = Limits {
    items: 1024,
    bytes: 8 * 1024 * 1024,
};

pub(super) struct Gate {
    snapshot: Snapshot,
    deadline: Option<Instant>,
    acknowledged: Option<String>,
    pub(super) verified_path: Option<PathBuf>,
    committed: bool,
    frames: ByteQueue<Value>,
}
impl Codec {
    pub(crate) fn prepare_branch(&mut self, home: &Path) -> Result<()> {
        let super::super::History::Fork {
            source,
            cutoff,
            source_paths,
        } = &self.config.history
        else {
            return Ok(());
        };
        self.branch = Some(Gate {
            snapshot: Snapshot::capture(
                home,
                source,
                &self.config.id,
                cutoff,
                crate::runtime::branching::history::Locations::new(
                    source_paths,
                    &self.config.cwd,
                    &self.config.id,
                )?,
            )?,
            deadline: None,
            acknowledged: None,
            verified_path: None,
            committed: false,
            frames: ByteQueue::default(),
        });
        Ok(())
    }
    pub(super) fn branch_input(&mut self) {
        if let Some(gate) = &mut self.branch {
            gate.deadline.get_or_insert_with(|| Instant::now() + WAIT);
        }
    }
    pub(super) fn hold_branch_ack(&mut self, id: &str) -> Result<bool> {
        let Some(gate) = &mut self.branch else {
            return Ok(false);
        };
        if gate.verified_path.is_some() {
            return Ok(false);
        }
        if gate.acknowledged.replace(id.into()).is_some() {
            return Err(protocol());
        }
        Ok(true)
    }
    pub(super) fn hold_branch_frame(&mut self, value: &Value) -> Result<bool> {
        let Some(gate) = &mut self.branch else {
            return Ok(false);
        };
        if !self.turn.input.enqueued
            || (gate.committed && gate.frames.is_empty())
            || (!gate.committed && metadata(value))
        {
            return Ok(false);
        }
        let bytes = value.to_string().len();
        if !gate.frames.try_push(value.clone(), bytes, HELD) {
            return Err(crate::error::Error::output_limit(
                "Unverified Claude branch output exceeded its bound.",
            ));
        }
        Ok(true)
    }
    pub(crate) fn poll_branch(&mut self) -> Result<Vec<Value>> {
        self.poll_branch_at(Instant::now())
    }
    pub(in crate::runtime::provider::claude) fn poll_branch_at(
        &mut self,
        now: Instant,
    ) -> Result<Vec<Value>> {
        let Some(gate) = &mut self.branch else {
            return Ok(vec![]);
        };
        if !gate.committed
            && (gate.deadline.is_some_and(|deadline| now >= deadline)
                || self
                    .turn
                    .active
                    .as_ref()
                    .is_some_and(|turn| turn.interrupt_sent))
        {
            return Err(crate::runtime::branching::history::refused());
        }
        let ack = if gate.verified_path.is_some() {
            None
        } else if let Some(id) = gate.acknowledged.as_ref() {
            if let Some(path) = gate.snapshot.verify(id)? {
                gate.verified_path = Some(path);
                Some(id.clone())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(id) = ack {
            self.acknowledge(Some(id), &self.config.id.clone(), None)?;
        }
        let mut replies = Vec::new();
        for _ in 0..32 {
            let Some(gate) = &mut self.branch else {
                break;
            };
            if !gate.committed {
                break;
            }
            let Some(value) = gate.frames.pop_front() else {
                self.branch = None;
                break;
            };
            // These frames already passed receive's echo and input checks. Drain
            // directly so remaining held frames cannot requeue them out of order.
            let mut output = self.receive_inner(value)?;
            output.extend(self.resolve_native_controls()?);
            self.settle();
            self.remember_replies(&output)?;
            replies.extend(output);
        }
        Ok(replies)
    }
    pub(crate) fn commit_branch(&mut self, turn: &str) -> Result<()> {
        let Some(gate) = &mut self.branch else {
            return Ok(());
        };
        if gate.verified_path.is_none()
            || gate.acknowledged.as_deref() != Some(turn)
            || !self.turn.active.as_ref().is_some_and(|active| {
                active.id == turn && active.acknowledged && !active.interrupt_sent
            })
        {
            return Err(protocol());
        }
        gate.committed = true;
        Ok(())
    }
    pub(crate) fn branch_wait(&self, timeout: Duration) -> Duration {
        if self.branch.as_ref().is_some_and(|gate| gate.committed) {
            return Duration::ZERO;
        }
        if self
            .branch
            .as_ref()
            .is_some_and(|gate| gate.deadline.is_some())
        {
            timeout.min(POLL)
        } else {
            timeout
        }
    }
}
fn metadata(value: &Value) -> bool {
    super::wire::Decision::read(value).branch_metadata()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn original(value: &Value) -> bool {
        match value["type"].as_str() {
            Some("control_response" | "control_cancel_request") => true,
            Some("system") => value["subtype"] == "init",
            Some("user") => value["isReplay"] == true,
            Some("control_request") => {
                value["request"]["subtype"] == "mcp_message"
                    && matches!(
                        value["request"]["message"]["method"].as_str(),
                        Some(
                            "initialize"
                                | "notifications/initialized"
                                | "ping"
                                | "tools/list"
                                | "notifications/cancelled"
                        )
                    )
            }
            _ => false,
        }
    }

    #[test]
    fn branch_metadata_preserves_partial_and_future_frames() {
        let malformed = [
            Value::Null,
            json!(true),
            json!(1),
            json!("future"),
            json!([]),
            json!({}),
        ];
        for kind in [
            "control_response",
            "control_cancel_request",
            "system",
            "user",
            "control_request",
            "future",
        ] {
            for method in [
                "initialize",
                "notifications/initialized",
                "ping",
                "tools/list",
                "notifications/cancelled",
                "tools/call",
                "future",
            ] {
                let raw = json!({"type":kind,"subtype":"init","isReplay":true,
                    "request":{"subtype":"mcp_message","server_name":5,"message":{"method":method}}});
                assert_eq!(metadata(&raw), original(&raw));
                for path in [
                    "/type",
                    "/subtype",
                    "/isReplay",
                    "/request/subtype",
                    "/request/message/method",
                    "/request/message",
                    "/request",
                ] {
                    for replacement in &malformed {
                        let mut value = raw.clone();
                        *value.pointer_mut(path).unwrap() = replacement.clone();
                        assert_eq!(metadata(&value), original(&value), "{path}: {value}");
                    }
                }
            }
        }
        for value in malformed {
            assert_eq!(metadata(&value), original(&value));
        }
    }
}
