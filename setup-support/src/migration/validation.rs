//! Exact admission of prepared sources, receipts and recovery content.
use super::{Engine, PENDING, Receipt, Result, Step, invalid, preparation, snapshot, storage};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

impl Engine<'_> {
    pub(super) fn check_prepared_source(&self, receipt: &Receipt) -> Result<()> {
        self.check_receipt(receipt)?;
        for change in &receipt.files {
            if snapshot(&storage::scoped(self.root, &change.path)?)? != change.before {
                return Err(invalid(
                    "External edit during migration preparation; backup retained.",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn source_paths(&self, step: &dyn Step) -> Result<Vec<PathBuf>> {
        (self.owner)()?;
        step.validate_before(self.root)?;
        let mut paths = step.files(self.root)?;
        paths.sort();
        if paths.is_empty()
            || paths.windows(2).any(|p| p[0] == p[1])
            || !paths.iter().any(|p| p == self.marker)
        {
            return Err(invalid(
                "Migration requires unique file paths and its marker.",
            ));
        }
        for path in &paths {
            storage::relative(path)?;
            self.check_file_path(path)?;
        }
        self.database_paths(step, &paths)?;
        self.check_marker(self.root, step.from())?;
        Ok(paths)
    }

    pub(super) fn validate_recovery(&self, step: &dyn Step, receipt: &Receipt) -> Result<()> {
        let backup = self.admin.join("backups").join(&receipt.backup);
        let staged = self.admin.join("migrations").join(&receipt.stage);
        storage::directory(&backup, true)?;
        storage::directory(&staged, true)?;
        // Recheck every input before the first resumed write. A conflict in a
        // later page cannot silently permit earlier pages to advance on retry.
        for change in &receipt.files {
            if snapshot(&storage::scoped(&backup, &change.path)?)? != change.before
                || snapshot(&storage::scoped(&staged, &change.path)?)? != change.after
            {
                return Err(invalid("Migration backup or prepared content changed."));
            }
            let current = snapshot(&storage::scoped(self.root, &change.path)?)?;
            if current != change.before && current != change.after {
                return Err(invalid(
                    "External edit fences migration recovery; preserve the intent and edited \
                        file.",
                ));
            }
        }
        step.validate_before(&backup)?;
        self.check_databases(step, receipt, &backup)?;
        step.validate_after(&staged)?;
        self.check_marker(&backup, receipt.from)?;
        self.check_marker(&staged, receipt.to)?;
        let marker: hive_input::compatibility::Marker =
            storage::record(&self.root.join(self.marker))?;
        if marker.format != receipt.from && marker.format != receipt.to {
            return Err(invalid("Migration marker is inconsistent with its intent."));
        }
        if marker.format == receipt.to {
            for change in &receipt.files {
                if snapshot(&storage::scoped(self.root, &change.path)?)? != change.after {
                    return Err(invalid(
                        "Completed migration marker has incomplete content.",
                    ));
                }
            }
        }
        let expected: BTreeSet<_> = step.files(&backup)?.into_iter().collect();
        if expected != receipt.files.iter().map(|c| c.path.clone()).collect() {
            return Err(invalid(
                "Pending migration file set differs from its registered step.",
            ));
        }
        Ok(())
    }

    pub(super) fn check_receipt(&self, receipt: &Receipt) -> Result<()> {
        (self.owner)()?;
        let saved = (receipt.root_device, receipt.root_inode);
        let live = storage::identity(self.root)?;
        let same = saved == live
            || crate::files::identity::saved(
                saved,
                live,
                crate::files::identity::parent_device(self.root)?,
            )
            .is_match();
        if !same
            || receipt.from == 0
            || receipt.to <= receipt.from
            || receipt.files.is_empty()
            || receipt.backup != receipt.stage
            || !storage::valid_name(&receipt.backup, receipt.from, receipt.to)
        {
            return Err(invalid("Invalid migration identity."));
        }
        let mut paths = BTreeSet::new();
        for change in &receipt.files {
            storage::relative(&change.path)?;
            self.check_file_path(&change.path)?;
            if !paths.insert(&change.path) {
                return Err(invalid("Duplicate migration path."));
            }
            for state in change.before.iter().chain(change.after.iter()) {
                state.validate()?;
            }
        }
        let mut databases = BTreeSet::new();
        for database in &receipt.databases {
            storage::relative(&database.path)?;
            self.check_file_path(&database.path)?;
            database.snapshot.validate()?;
            if !paths.contains(&database.path) || !databases.insert(&database.path) {
                return Err(invalid("Invalid migration database backup list."));
            }
        }
        if !paths.contains(&self.marker.to_path_buf()) {
            return Err(invalid("Migration marker is missing."));
        }
        Ok(())
    }

    pub(super) fn check_file_path(&self, path: &Path) -> Result<()> {
        let target = self.root.join(path);
        if target == self.admin.join(PENDING)
            || target.starts_with(self.admin.join("backups"))
            || target.starts_with(self.admin.join("migrations"))
            || path.starts_with(preparation::DATABASES)
        {
            return Err(invalid("A format step cannot rewrite migration evidence."));
        }
        Ok(())
    }

    pub(super) fn check_marker(&self, root: &Path, expected: u32) -> Result<()> {
        let marker: hive_input::compatibility::Marker =
            storage::record(&storage::scoped(root, self.marker)?)?;
        if marker.format != expected {
            return Err(invalid(
                "Migration marker disagrees with its registered step.",
            ));
        }
        Ok(())
    }
}
