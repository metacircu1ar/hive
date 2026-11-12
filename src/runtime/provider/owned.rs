//! Receipt-owned native RPC launch records authorization and definite spawn absence.
use super::{Backend, Rpc};
use crate::{
    error::{Error, Result},
    launch::LauncherCache,
    state::ChildReceipt,
};
use std::{os::fd::BorrowedFd, process::Command};

pub(crate) struct RpcOwnership<'a> {
    pub receipt: &'a ChildReceipt,
    pub owner: Option<BorrowedFd<'a>>,
}
pub(crate) fn start_owned_rpc(
    launcher: &LauncherCache,
    command: Command,
    backend: Backend,
    ownership: RpcOwnership<'_>,
    authorize: impl FnOnce() -> Result<()>,
) -> Result<Rpc> {
    let RpcOwnership { receipt, owner } = ownership;
    let inherited = std::iter::once(Some(receipt.fd()))
        .chain(owner.map(Some))
        .collect();
    match Rpc::start_authorized(launcher, command, backend, inherited, |pid| {
        receipt.started(pid)?;
        authorize()
    }) {
        Ok(rpc) => Ok(rpc),
        Err(error) => {
            let cleanup = if matches!(error, Error::SpawnFailed { .. }) {
                receipt.not_started()
            } else {
                Ok(())
            };
            crate::observability::settle(Err(error), cleanup, "provider_not_started")
        }
    }
}
