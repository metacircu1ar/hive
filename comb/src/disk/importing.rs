//! Reconfirm a page receipt before advancing import progress.
use super::{Disk, durable::sync_existing};
use crate::Result;

impl Disk {
    pub(crate) fn durable_import_receipt(&self, receipt: &crate::page::Receipt) -> Result<()> {
        sync_existing(
            &self.receipt_path(receipt.principal.as_str(), receipt.request_id.as_str())?,
            self.durability.as_ref(),
            true,
        )
    }
}
