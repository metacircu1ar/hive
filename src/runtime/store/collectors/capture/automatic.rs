use super::{CollectionSource, projection, records};
use crate::runtime::domain::{Agent, EventKind};
use serde_json::Value;

pub(in crate::runtime::store) fn automatic<'a>(
    agent: &'a Agent,
    kind: EventKind,
    data: &'a Value,
) -> Option<(CollectionSource, records::Automatic<'a>)> {
    match kind {
        EventKind::ExecutionStopped => {
            let recovery: projection::Recovery<'_> = projection::read(data);
            if recovery.reason != Some("daemon_restarted") || recovery.outcome != Some("uncertain")
            {
                return None;
            }
            Some(outcome(
                agent,
                CollectionSource::TurnFailed,
                "uncertain",
                recovery.turn_id,
                None,
            ))
        }
        EventKind::TaskCompleted => {
            let turn: projection::Turn<'_> = projection::read(data);
            Some(outcome(
                agent,
                CollectionSource::TaskCompleted,
                "completed",
                turn.turn_id,
                None,
            ))
        }
        EventKind::ExecutionFailed => {
            let failure: projection::Failure<'_> = projection::read(data);
            let code = projection::resolve(failure.error.code, |result| {
                crate::observability::optional(result, "collector_error_projection")
            })?;
            Some(outcome(
                agent,
                CollectionSource::TurnFailed,
                "failed",
                failure.turn_id,
                Some(code),
            ))
        }
        EventKind::TurnFinished => {
            let turn: projection::Turn<'_> = projection::read(data);
            let status = turn
                .status
                .filter(|status| matches!(*status, "failed" | "interrupted"))?;
            Some(outcome(
                agent,
                CollectionSource::TurnFailed,
                status,
                turn.turn_id,
                None,
            ))
        }
        EventKind::ApprovalClosed => approval(agent, data),
        _ => None,
    }
}

fn outcome<'a>(
    agent: &'a Agent,
    source: CollectionSource,
    outcome: &'a str,
    turn: Option<&'a str>,
    error_code: Option<crate::error::ErrorCode>,
) -> (CollectionSource, records::Automatic<'a>) {
    (
        source,
        records::Automatic::Agent {
            source,
            actor: &agent.id,
            role: agent.role.as_deref(),
            execution: agent.execution_id.as_deref(),
            turn,
            evidence: records::Outcome {
                outcome,
                error_code,
            },
        },
    )
}

fn approval<'a>(
    agent: &'a Agent,
    data: &'a Value,
) -> Option<(CollectionSource, records::Automatic<'a>)> {
    let view: projection::Answer<'_> = projection::read(data);
    let action = projection::resolve(view.action, |result| {
        crate::observability::optional(result, "collector_approval_projection")
    })?;
    let source = CollectionSource::ApprovalAnswer;
    Some((
        source,
        records::Automatic::Approval {
            source,
            actor: "human",
            subject_agent: &agent.id,
            subject_role: agent.role.as_deref(),
            execution: view.origin.execution,
            turn: view.origin.turn,
            approval_id: view.approval_id,
            evidence: records::Approval { action },
        },
    ))
}
