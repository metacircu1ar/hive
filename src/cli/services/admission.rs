//! Share selected-path observations with Comb before manager discovery.
use super::{Result, service};
use crate::cli::input::Context;

pub(super) fn validate(command: &service::Command) -> Result<()> {
    hive_setup_support::service::admission::validate(command).map_err(|failure| {
        Context::new("service install", failure.field)
            .selected_path(failure.cause, failure.expected)
            .error
    })
}
