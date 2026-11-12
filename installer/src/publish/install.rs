//! Stage one managed installation, then publish or restore its owned links.
use super::{
    Activation, BTreeMap, Installed, Links, Manifest, Path, PathBuf, Report, Result, current, load,
    prepare_links, publication, remove_links, rollback, stage_generation, sync_parent, usage,
};
use crate::lifecycle::{Change, Lifecycle, Operation};

pub(super) struct Installation<'a> {
    pub prefix: &'a Path,
    pub source: &'a Path,
    pub selected: &'a [Manifest],
    pub manager: &'a Path,
    pub target: &'a str,
    pub installer: &'a publication::Image,
}

impl Installation<'_> {
    pub(super) fn run(
        self,
        lifecycle: &mut dyn Lifecycle,
        mut hook: impl FnMut(&str) -> Result<()>,
    ) -> Result<Option<Report>> {
        lifecycle.recover(self.prefix, self.manager)?;
        let old = current(self.manager)?;
        let mut installed = load(self.manager, old.as_deref(), self.target)?;
        let (expected, missing) =
            prepare_links(self.prefix, &mut installed, self.selected, &mut hook)?;
        // Interrupted attempts may have created entries without syncing parents.
        sync_parent(&self.prefix.join("lib"), &mut hook)?;
        sync_parent(self.manager, &mut hook)?;
        let generation = stage_generation(
            self.manager,
            &mut installed,
            super::Generation {
                source: self.source,
                selected: self.selected,
                previous: old.as_deref(),
                installer: self.installer,
            },
        )?;
        tracing::info!(name: "install.generation_staged", success = true);
        let pointer = generation.strip_prefix(self.manager)?.to_owned();
        let staged = Staged {
            prefix: self.prefix,
            manager: self.manager,
            old,
            installed,
            expected,
            missing,
            generation,
            pointer,
        };
        staged.prepare(self.selected, lifecycle)?;
        staged.publish(lifecycle, &mut hook)?;
        Ok(installed_usage(self.manager))
    }
}

struct Staged<'a> {
    prefix: &'a Path,
    manager: &'a Path,
    old: Option<PathBuf>,
    installed: Installed,
    expected: Links,
    missing: Vec<PathBuf>,
    generation: PathBuf,
    pointer: PathBuf,
}

impl Staged<'_> {
    fn prepare(&self, selected: &[Manifest], lifecycle: &mut dyn Lifecycle) -> Result<()> {
        let preparation = lifecycle.prepare(&Change {
            operation: Operation::Install,
            removed_links: BTreeMap::new(),
            prefix: self.prefix.into(),
            manager: self.manager.into(),
            previous: self.old.clone(),
            next: self.pointer.clone(),
            added_links: self
                .missing
                .iter()
                .map(|name| (name.clone(), self.expected[name].clone()))
                .collect(),
            components: selected
                .iter()
                .map(|p| p.component().map(str::to_owned))
                .collect::<Result<_>>()?,
        });
        if let Err(error) = preparation {
            let cleanup = publication::discard_unreferenced(self.manager, &self.generation);
            return Err(publication::failed(
                error,
                "Preparation cleanup failed",
                cleanup,
            ));
        }
        Ok(())
    }

    fn publish(
        &self,
        lifecycle: &mut dyn Lifecycle,
        hook: &mut impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        let mut activation = Activation {
            prefix: self.prefix,
            manager: self.manager,
            pointer: &self.pointer,
            expected: &self.expected,
            missing: &self.missing,
            created: Vec::new(),
        };
        let result = (|| {
            lifecycle.stop()?;
            activation.run(hook)?;
            for part in self.installed.parts.values() {
                part.check_versions(&self.prefix.join("lib/hive/current"))?;
            }
            tracing::info!(name: "install.versions_verified", success = true);
            Ok(())
        })();
        if let Err(error) = result {
            return Err(self.restore(error, &activation.created, lifecycle, hook));
        }
        lifecycle.finish(true).map_err(|error| {
            publication::context(
                "New binaries are installed, but service restart is incomplete. Repeat the \
                installation to recover",
                error,
            )
        })
    }

    fn restore(
        &self,
        error: crate::Error,
        created: &[PathBuf],
        lifecycle: &mut dyn Lifecycle,
        hook: &mut impl FnMut(&str) -> Result<()>,
    ) -> crate::Error {
        if let Err(uncertain) = rollback(self.manager, self.old.as_deref(), hook) {
            return publication::failed(
                error,
                "Rollback uncertain; generations and links retained",
                Err(uncertain),
            );
        }
        let cleanup = (|| {
            // A failed link cleanup must not skip restoring unrelated services.
            let restored = lifecycle.restore(false);
            let links = remove_links(self.prefix, created, &self.expected);
            crate::lifecycle::settle(restored, links)?;
            // Clear the journal before deleting anything it may still reference.
            lifecycle.clear()?;
            publication::discard_unreferenced(self.manager, &self.generation)
        })();
        publication::failed(
            error,
            "Rollback cleanup or service restoration failed",
            cleanup,
        )
    }
}

// A failed advisory count is unknown, never a fabricated zero or failed install.
fn installed_usage(manager: &Path) -> Option<Report> {
    match usage(&manager.join("generations")) {
        Ok(report) => Some(report),
        Err(error) => {
            tracing::warn!(name: "install.usage_unavailable",
                 error = error.as_ref() as &(dyn std::error::Error + 'static),
                 decision = "retain_successful_installation");
            None
        }
    }
}
