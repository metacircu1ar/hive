//! Human import is atomic per page, with one retained resumable batch.
mod admin;
mod input;
pub(crate) mod journal;
mod progress;
mod publication;
mod run;

use crate::{Comb, Error, Result, values::Slug};
use comb_protocol::import::ImportReport;
use input::Inputs;
use journal::Record;
use std::path::Path;

impl Comb {
    pub fn import(&mut self, source: &Path, prefix: Option<&Slug>) -> Result<ImportReport> {
        self.ready()?;
        let record = self.disk.import_record()?;
        let inputs = Inputs::read(source, prefix).map_err(|mut error| {
            if let Some(Record::Pending(batch)) = &record {
                error.message = Error::import_source_changed().message;
                return match batch.confirmed(&self.disk) {
                    Ok(done) => batch.failure(error, &done, None),
                    Err(receipt_error) => {
                        tracing::warn!(name: "comb.import_progress_unavailable",
                             primary = &error as &(dyn std::error::Error + 'static),
                             secondary = &receipt_error as &(dyn std::error::Error + 'static),
                             decision = "retain_primary_and_receipts");
                        error.committed()
                    }
                };
            }
            error
        })?;
        match record {
            Some(Record::Pending(batch)) => {
                let done = batch.confirmed(&self.disk)?;
                let matches = batch
                    .matches(&inputs, prefix)
                    .map_err(|error| batch.failure(error, &done, None))?;
                if !matches {
                    return Err(batch.failure(Error::import_pending(), &done, None));
                }
                self.resume_import(batch, done)
            }
            Some(Record::Completed(batch))
                if batch.matches(&inputs, prefix).unwrap_or_else(|error| {
                    tracing::warn!(name: "comb.completed_import_identity_unavailable",
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "prepare_new_import");
                    false
                }) =>
            {
                Ok(batch.report())
            }
            Some(Record::Completed(_) | Record::Abandoned(_)) | None => {
                let batch = self.prepare_import(inputs, prefix)?;
                let record = Record::Pending(batch);
                if let Err(error) = self.disk.publish_import(&record) {
                    if !error.file_committed {
                        return Err(error);
                    }
                    let Record::Pending(batch) = &record else {
                        return Err(error);
                    };
                    return Err(batch.failure(error, &vec![false; batch.entries.len()], None));
                }
                let Record::Pending(batch) = record else {
                    return Err(Error::invalid_import_journal());
                };
                let done = vec![false; batch.entries.len()];
                self.resume_import(batch, done)
            }
        }
    }
}

#[cfg(test)]
mod tests;
