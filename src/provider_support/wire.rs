//! Shared native provider method vocabulary and outgoing frames.
mod methods;
pub(crate) use methods::{ClientMethod, ServerMethod};
pub(crate) mod codes;
pub(crate) mod outgoing;
