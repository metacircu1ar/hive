//! Expected form validation failures, rendered only when presenting the card.
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::cli::attach) enum AnswerError {
    Required(String),
    Array(String),
    Integer(String),
    Number(String),
    Schema,
}

impl fmt::Display for AnswerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Required(name) => write!(formatter, "{name} is required"),
            Self::Array(name) => write!(formatter, "{name} must be a JSON array"),
            Self::Integer(name) => write!(formatter, "{name} must be a whole number"),
            Self::Number(name) => write!(formatter, "{name} must be a number"),
            Self::Schema => formatter.write_str("the answer does not match the question's schema"),
        }
    }
}

impl std::error::Error for AnswerError {}
