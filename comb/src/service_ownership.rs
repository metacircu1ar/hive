//! Inspect service ownership without depending on service orchestration.
use crate::Result;
use hive_setup_support::service::{Layout, Scope};
use std::path::Path;

pub fn scope(root: &Path) -> std::io::Result<Scope> {
    Scope::new("comb", &[root])
}
pub fn owned(root: &Path) -> Result<bool> {
    Ok(hive_setup_support::service::load(&Layout::discover()?, &scope(root)?)?.is_some())
}
