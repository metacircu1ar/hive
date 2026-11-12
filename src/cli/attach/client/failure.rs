//! Typed command failures retain codes until the view renders their message.
use crate::error::ErrorCode;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::cli::attach) struct Failure {
    pub code: Option<ErrorCode>,
    message: String,
}
impl Failure {
    pub(super) fn refused(code: ErrorCode, message: String) -> Self {
        Self {
            code: Some(code),
            message,
        }
    }
    pub(super) fn exhausted() -> Self {
        Self::refused(
            ErrorCode::DaemonUnavailable,
            "the daemon did not answer; retry later".into(),
        )
    }
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = self.code {
            write!(formatter, "{}: {}", code.as_str(), self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}
impl std::error::Error for Failure {}
