//! Shared validation for the error envelope and its typed metadata values.
use super::{Error, Result};

pub(crate) fn request_id(value: &str) -> Result<()> {
    hive_input::validate::request_id(value)
        .map_err(|cause| Error::invalid_request_id().caused_by(cause))
}
