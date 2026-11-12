//! Separate credential usability from per-execution cleanup and conversation fences.
use super::{
    Binding, Lock, State,
    identity::{LockIdentity, ProcessIdentity},
};
use crate::error::{Error, Result};
use serde::Serialize;
use std::{io::ErrorKind, path::Path};
use uuid::Uuid;
const FINISH_LOGIN_FIRST: &str = "Finish auth login first.";

mod cleanup;
mod credential;
pub(super) mod layout;
pub(super) mod ledger;
mod namespace;
mod operations;
mod probes;
pub(in crate::state) use ledger::migration_records;
pub(in crate::state) use probes::migration_probes;
mod report;
pub use report::RecoveryReport;
#[cfg(test)]
mod tests;

pub struct ProcessCompletion {
    pub quiescent: bool,
    pub confirmed: bool,
}

pub use ledger::{ProviderEvidence, RuntimeLease};
pub use operations::{ChildAccess, ChildReceipt, Operation};

pub use crate::error::recovery_required;

#[derive(Serialize)]
pub struct RecoveryReservation {
    pub auth: String,
    pub provider: String,
    pub process: Option<ProcessIdentity>,
    pub gone: bool,
    pub unknown_capacity: bool,
    pub unresolved_activity: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    Status,
    Models,
    SteeringCanary,
    Login,
    Recover,
    Prune,
}
impl OperationKind {
    pub(super) fn is_reader(self) -> bool {
        matches!(self, Self::Status | Self::Models | Self::SteeringCanary)
    }
}

impl State {
    pub fn runtime_lease(&self, binding: &Binding) -> Result<RuntimeLease> {
        ledger::lease(self, binding)
    }
    pub fn operation(&self, binding: &Binding, kind: OperationKind) -> Result<Operation> {
        operations::begin(self, binding, kind)
    }
    pub fn recovery_report(&self, binding: &Binding) -> Result<RecoveryReport> {
        let _admission = self.admission()?;
        self.check_format()?;
        match ledger::reconcile(self, binding) {
            Ok(report) => Ok(RecoveryReport::Snapshot(report)),
            Err(
                error @ (Error::UnsupportedSchema { .. } | Error::RegistrationIncomplete { .. }),
            ) => Err(error),
            Err(error) => Ok(RecoveryReport::diagnostic(binding.id, error)),
        }
    }
    pub fn check_conversation(&self, binding: &Binding, provider: Option<&str>) -> Result<()> {
        let Some(provider) = provider else {
            return Ok(());
        };
        let _admission = self.admission()?;
        self.check_format()?;
        if ledger::conversation_safe(binding, provider)? {
            return Ok(());
        }
        Err(Error::recovery_required(concat!(
            "This conversation has unresolved execution or tool cleanup. Fresh agents",
            " can use the profile; inspect or terminate this agent, or resume only ",
            "when its original execution is proven safe. Native conversation ",
            "rebinding is unsupported.",
        )))
    }
    pub fn recovery_reservations(&self) -> Result<Vec<RecoveryReservation>> {
        let _admission = self.admission()?;
        self.check_format()?;
        let bindings: Vec<_> = {
            let _metadata = if exists(&self.root.join("index.lock"))? {
                Some(self.metadata_lock()?)
            } else {
                None
            };
            let index = self.read_index()?;
            index
                .generations
                .into_values()
                .filter(|g| g.state != super::GenerationState::Pruned)
                .map(|g| (g.profile_name, g.binding))
                .collect()
        };
        Ok(bindings
            .into_iter()
            .flat_map(|(name, binding)| ledger::reservations(&binding, &name))
            .collect())
    }
}

pub(super) fn exists(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::state_io_from(e)),
    }
}
