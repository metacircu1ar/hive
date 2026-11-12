//! Atomic fan-out with optional durable collection of each consuming turn's reply.
use super::{Caller, Core, Facts, Request, RequestEnvelope, api, invalid_request};
use crate::{
    error::{Error, Result},
    runtime::{
        api::{Input, batch, joins::JoinAccepted},
        domain::DeliveryMode,
    },
};
use std::collections::HashSet;

pub(super) struct BatchRequest<'a> {
    pub id: &'a str,
    pub args: &'a api::BatchArgs,
    pub envelope: &'a RequestEnvelope,
}

impl Core {
    pub(super) fn batch(
        &mut self,
        caller: &Caller,
        request: BatchRequest<'_>,
        facts: &Facts,
    ) -> Result<api::BatchResponse<api::Response>> {
        let BatchRequest {
            id: request_id,
            args,
            envelope,
        } = request;
        let items = envelope.checked_batch_items()?;
        let join = self.batch_join(caller, request_id, args)?;
        let mut results = Vec::new();
        let mut members = Vec::new();
        let mut ids = HashSet::new();
        for (index, item) in items.iter().enumerate() {
            let operation = item
                .operation()
                .map_err(|error| batch::at_index(error, index))?;
            distinct_outer_id(&operation, request_id)
                .map_err(|error| batch::at_index(error, index))?;
            let api::BatchOperation { id, request } = &operation;
            if join.is_some() {
                self.validate_join_member(caller, &operation, request_id, &mut ids)
                    .map_err(|error| batch::at_index(error, index))?;
            }
            let response = self
                .dispatch(caller, id, request, facts)
                .map_err(|error| batch::at_index(error, index))?;
            if let Some(join) = &join {
                let message = joined_message(&request.request, &response)?;
                if let Some(message) = message {
                    let task_scoped = match &request.request {
                        Request::Spawn(args) => args.one_shot()?,
                        _ => false,
                    };
                    self.store.join_member(
                        join,
                        caller.name(),
                        crate::runtime::store::MemberInput {
                            message: &message,
                            position: members.len(),
                            task_scoped,
                        },
                    )?;
                    members.push(message);
                }
            }
            results.push(response);
        }
        // An inner request may have become replayable after preflight. Its failed
        // capture must not allow other, deliberately unpublished snapshots to commit.
        facts.attachments_published()?;
        let join = self.finish_batch_join(caller, request_id, join, members)?;
        Ok(api::BatchResponse { results, join })
    }

    fn finish_batch_join(
        &self,
        caller: &Caller,
        request_id: &str,
        join: Option<String>,
        members: Vec<String>,
    ) -> Result<Option<JoinAccepted>> {
        if let Some(join) = &join {
            if members.is_empty() {
                return Err(batch::error(
                    hive_input::Code::ConflictingArguments,
                    "/operations",
                    "a joined batch with at least one spawn or send operation",
                ));
            }
            self.store
                .join_requested(join, caller.name(), request_id, members.len())?;
        }
        Ok(join.map(|join_id| JoinAccepted { join_id, members }))
    }

    fn batch_join(
        &self,
        caller: &Caller,
        request_id: &str,
        args: &api::BatchArgs,
    ) -> Result<Option<String>> {
        let join_options = args.join_options()?;
        if join_options.is_some()
            && !caller.is_human()
            && self
                .store
                .agent(caller.name())?
                .completion_pending
                .is_some()
        {
            return Err(Error::completion_pending(
                "Completion is pending the turn boundary; a join started now would be cancelled.",
            ));
        }
        join_options
            .map(|options| {
                self.store.create_join(
                    caller.name(),
                    request_id,
                    options.deadline(crate::state::now())?,
                )
            })
            .transpose()
    }

    fn validate_join_member(
        &self,
        caller: &Caller,
        operation: &api::BatchOperation,
        outer: &str,
        ids: &mut HashSet<String>,
    ) -> Result<()> {
        let api::BatchOperation { id, request } = operation;
        let request = &request.request;
        #[cfg(not(feature = "comb"))]
        if let Request::Send(send) = request {
            crate::runtime::features::require(&self.store.relative_target(
                caller,
                send.target.required(api::InputContext::Send, "/target")?,
                api::InputContext::Send,
            )?)?;
        }
        if id == outer
            || !ids.insert(id.to_owned())
            || self.store.request_used(caller.name(), id)?
        {
            return Err(Error::request_conflict(
                "A new joined batch requires fresh, distinct operation request IDs.",
            ));
        }
        if let Request::Send(send) = request
            && (!matches!(
                send.mode,
                Input::Missing | Input::Valid(DeliveryMode::NextTurn)
            ) || send.target.as_str() == Some(caller.name()))
        {
            return Err(batch::error(
                hive_input::Code::ConflictingArguments,
                if send.target.as_str() == Some(caller.name()) {
                    "/target"
                } else {
                    "/mode"
                },
                "join sends require next-turn delivery to another agent",
            ));
        }
        Ok(())
    }
}

fn joined_message(request: &Request, response: &api::Response) -> Result<Option<String>> {
    let message = if matches!(request, Request::Send(_) | Request::Spawn(_)) {
        Some(
            response
                .message_id()
                .ok_or_else(invalid_request)?
                .to_owned(),
        )
    } else {
        None
    };
    Ok(message)
}

fn distinct_outer_id(operation: &api::BatchOperation, outer: &str) -> Result<()> {
    if operation.id == outer {
        return Err(Error::request_conflict(
            "Batch operations must not reuse the batch request ID.",
        ));
    }
    Ok(())
}
