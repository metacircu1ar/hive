//! Native selected paths are checked before service-manager discovery.
use crate::{Error, Result};
use hive_setup_support::service::{Command, admission};

pub(super) fn validate(command: &Command) -> Result<()> {
    admission::validate(command).map_err(|failure| {
        let Some(code) = admission::input_code(failure.field, &failure.cause) else {
            return Error::from(failure.cause);
        };
        match hive_input::InputError::new(
            code,
            "Cannot use the selected path.",
            failure.field,
            failure.expected,
            vec!["comb service install".into()],
        ) {
            Ok(input) => Error::from(input).caused_by(failure.cause),
            Err(cause) => Error::internal_serialization().caused_by(cause),
        }
    })
}
