//! Compare a durable filesystem observation with a freshly validated object.
//! Live descriptor/name checks must continue to compare device and inode exactly.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    Exact,
    Renumbered,
    Mismatch,
}

impl Match {
    pub fn is_match(self) -> bool {
        if self == Self::Renumbered {
            tracing::debug!(name: "files.saved_device_renumbered",
                decision = "match_inode_and_parent");
        }
        self != Self::Mismatch
    }
}

/// `parent_device` belongs to the object's already-validated live containing
/// directory. A different live device is a mount boundary, not renumbering.
/// Callers with a saved boot identity must require an exact match on that boot.
/// This cannot distinguish an inode-preserving clone of the entire volume.
pub fn saved(saved: (u64, u64), live: (u64, u64), parent_device: u64) -> Match {
    if saved == live {
        Match::Exact
    } else if saved.1 == live.1 && live.0 == parent_device {
        Match::Renumbered
    } else {
        Match::Mismatch
    }
}

/// Read a containing directory using the setup ancestor ownership policy.
/// Product callers must additionally retain their own namespace/marker checks.
pub fn parent_device(path: &std::path::Path) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    let parent = path
        .parent()
        .ok_or_else(|| crate::invalid("Object needs a parent."))?;
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || !crate::directories::ancestor(
            metadata.uid(),
            metadata.mode(),
            rustix::process::geteuid().as_raw(),
        )
    {
        return Err(crate::invalid("Untrusted identity parent."));
    }
    Ok(metadata.dev())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_identity_requires_inode_and_live_parent_filesystem() {
        assert_eq!(saved((1, 2), (1, 2), 3), Match::Exact);
        assert_eq!(saved((1, 2), (3, 2), 3), Match::Renumbered);
        assert_eq!(saved((1, 2), (3, 2), 4), Match::Mismatch);
        assert_eq!(saved((1, 2), (1, 4), 1), Match::Mismatch);
        assert_eq!(saved((1, 2), (3, 4), 3), Match::Mismatch);
    }
}
