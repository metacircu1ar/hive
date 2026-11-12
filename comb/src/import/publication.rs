//! Record validation and publication belong to the import recovery flow.
use super::journal::{MAX_BYTES, Record};
use crate::{
    Error, Result,
    disk::{Disk, atomic_admin, exists, read, sync_import_admin},
};

impl Disk {
    pub(crate) fn import_record(&self) -> Result<Option<Record>> {
        let path = self.admin.join("import.json");
        if !exists(&path)? {
            return Ok(None);
        }
        let record: Record = hive_input::decode::from_slice(&read(&path, MAX_BYTES)?)?;
        record.validate(self)?;
        sync_import_admin(&path, self.durability().as_ref())
            .map_err(|error| record.failure(error, self))?;
        Ok(Some(record))
    }
    pub(crate) fn publish_import(&self, record: &Record) -> Result<()> {
        record.validate(self)?;
        let bytes = crate::error::json_bytes(record)?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::file_size_limit());
        }
        atomic_admin(
            &self.admin.join("import.json"),
            &bytes,
            self.durability().as_ref(),
        )
    }
}
