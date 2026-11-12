//! Launch-only MCP configuration belongs to the private shell-control owner.
use super::{McpConfiguration, ShellGuard};
use crate::{config::McpServers, error::Result};
use std::path::PathBuf;

pub(super) fn publish(servers: &McpServers, guard: ShellGuard) -> Result<(ShellGuard, PathBuf)> {
    let path = guard.directory().join("mcp.json");
    let bytes = crate::api::json_text(McpConfiguration::new(servers));
    // Regenerated for each native process, never an authoritative state record.
    let result = atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_durability(atomic_fs::Durability::AtomicVisible),
    )
    .create_new(&path, bytes.as_bytes());
    match result {
        Ok(()) => Ok((guard, path)),
        Err(error) => {
            if error.retained().is_some() {
                guard.retain_preparation();
            }
            Err(crate::publication::error(error))
        }
    }
}
