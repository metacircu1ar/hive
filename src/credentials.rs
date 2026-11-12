//! Captured credentials, opaque secrets and shared freshness ownership.
mod credential;
mod key;
mod keys;
pub(crate) use credential::{ApiLogin, Credential};
pub(crate) use keys::Keys;

pub(crate) fn present(profile: &crate::config::Profile) -> crate::error::Result<()> {
    key::resolve(profile).map(|_| ())
}
