//! Validate delayed sends and promote durable timers through the writer transaction.
use super::{
    Core,
    api::{Input, InputContext as Op, SendArgs},
    domain::{AgentState, DeliveryMode, MessageState},
    store::ScheduledProof,
};
use crate::error::{Error, Result};
use std::time::Duration;

use crate::limits::MAX_DELAY;

pub(crate) fn delay(args: &SendArgs) -> Result<Option<u64>> {
    if args.delay_seconds.is_missing() {
        return Ok(None);
    }
    let seconds = args
        .delay_seconds
        .optional(
            Op::Send,
            "/delay_seconds",
            "an integer from 1 to 604800 seconds",
        )?
        .copied()
        .filter(|n| (1..=MAX_DELAY).contains(n))
        .ok_or_else(|| {
            Op::Send.error(
                hive_input::Code::OutOfRange,
                "/delay_seconds",
                "an integer from 1 to 604800 seconds",
            )
        })?;
    if !matches!(&args.mode, Input::Missing) && args.mode.copied() != Some(DeliveryMode::NextTurn) {
        return Err(Op::Send.error(
            hive_input::Code::ConflictingArguments,
            "/mode",
            "next-turn for a delayed message",
        ));
    }
    Ok(Some(seconds))
}

pub(crate) fn not_before(now: Duration, delay: u64) -> Result<u64> {
    now.as_secs()
        .checked_add(u64::from(now.subsec_nanos() != 0))
        .and_then(|n| n.checked_add(delay))
        .filter(|n| i64::try_from(*n).is_ok())
        .ok_or_else(|| {
            Error::invalid_request("Message deadline exceeds the supported timestamp range.")
        })
}

impl Core {
    pub(crate) fn promote_scheduled(&mut self, now: u64) -> Result<usize> {
        self.atomic(|core| {
            let now = i64::try_from(now).map_err(|cause| {
                Error::database_error("Invalid deadline clock.").with_source(cause)
            })?;
            let due = core.store().due_scheduled(now)?;
            let count = due.len();
            for row in due {
                core.atomic(|core| {
                    let mut agent = core.store().agent(&row.recipient)?;
                    let ScheduledProof { valid, uncertain } =
                        core.store().scheduled_proof(&row.id, &row.sender)?;
                    if !valid {
                        core.store().quarantine_scheduled(&row, uncertain)?;
                        return Ok(());
                    }
                    let state = if agent.terminal() {
                        MessageState::Undeliverable
                    } else {
                        MessageState::Accepted
                    };
                    let seq = core.store().activate_scheduled(&row, state)?;
                    if state == MessageState::Accepted {
                        if agent.state == AgentState::Stopped && !agent.paused {
                            agent.state = AgentState::Starting;
                            core.store().save(&agent)?;
                        }
                        core.store().record_promotion(&agent, &row, seq)?;
                    }
                    Ok(())
                })?;
            }
            Ok(count)
        })
    }
}
