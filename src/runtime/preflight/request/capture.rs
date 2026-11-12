//! Capture original input identities and filesystem facts before resolution.
use super::{
    Caller, Core, OperationInput, Request, RequestEnvelope, RequestInput, Result, Source,
    attachment_preflight,
};

struct OperationCapture<'a> {
    core: &'a Core,
    caller: &'a Caller,
    operations: Vec<OperationInput>,
}

impl OperationCapture<'_> {
    fn capture(core: &Core, caller: &Caller, envelope: &RequestEnvelope) -> Vec<OperationInput> {
        let mut capture = OperationCapture {
            core,
            caller,
            operations: Vec::new(),
        };
        capture.append(envelope);
        capture.operations
    }

    fn append(&mut self, envelope: &RequestEnvelope) {
        match &envelope.request {
            Request::Spawn(_) | Request::Resume(_) => {
                self.operations.push(OperationInput {
                    key: envelope.fingerprint(),
                    request: envelope.request.clone(),
                });
            }
            Request::Send(args) if attachment_preflight(&args.attachments) => {
                self.operations.push(OperationInput {
                    key: envelope.fingerprint(),
                    request: envelope.request.clone(),
                });
            }
            Request::Batch(_) => {
                // Invalid batches fail at the dispatcher without needing external facts.
                if let Some(items) = envelope.batch_items() {
                    for item in items {
                        if let Ok(item) = item.operation() {
                            // A replay or conflict is resolved lazily by dispatch. Neither
                            // needs to wait for external files again.
                            if matches!(
                                self.core.replay(self.caller, &item.id, &item.request),
                                Ok(None)
                            ) {
                                self.append(&item.request);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
fn capture_source(core: &Core, id: &str) -> Result<Source> {
    let agent = core.store().agent(id)?;
    crate::runtime::features::require(&agent)?;
    let instructions = core.store().instructions(id)?;
    Ok(Source {
        agent,
        instructions,
    })
}

impl RequestInput {
    pub(in crate::runtime) fn capture(
        core: &Core,
        caller: &Caller,
        envelope: &RequestEnvelope,
    ) -> Self {
        let operations_list = OperationCapture::capture(core, caller, envelope);
        let resumes = operations_list
            .iter()
            .filter_map(|operation| match &operation.request {
                Request::Resume(args) => args
                    .target
                    .as_str()
                    .map(|id| (id.to_owned(), core.store().agent(id))),
                _ => None,
            })
            .collect();
        let sources = operations_list
            .iter()
            .filter_map(|operation| match &operation.request {
                Request::Spawn(args) => args
                    .from_agent
                    .as_str()
                    .filter(|_| {
                        args.from_agent
                            .required(super::super::super::api::InputContext::Spawn, "/from_agent")
                            .is_ok()
                    })
                    .map(|id| (id.to_owned(), capture_source(core, id))),
                _ => None,
            })
            .collect();
        Self {
            attachment_root: core.store().attachments_dir.clone(),
            attachment_guard: None,
            fs: core.fs_context(),
            parent: caller
                .agent_id()
                .map(|id| core.store().agent(id))
                .transpose(),
            operations: operations_list,
            resumes,
            sources,
        }
    }
}
