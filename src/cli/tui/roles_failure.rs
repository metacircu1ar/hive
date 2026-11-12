//! Roles lookup failures retain the daemon code until the dialog renders it.
use crate::error::ErrorCode;
use std::fmt;

pub(super) enum RolesFailure {
    Refused(ErrorCode),
    Unavailable,
}
impl fmt::Display for RolesFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(code) => formatter.write_str(code.as_str()),
            Self::Unavailable => formatter.write_str("the daemon did not answer"),
        }
    }
}
