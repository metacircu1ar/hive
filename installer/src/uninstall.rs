//! Removing active entrypoints is a journaled publication, not generation GC.
use crate::{
    Result,
    lifecycle::{Change, Lifecycle, Operation},
    manifest, publish, services,
};
use hive_setup_support::files;
#[cfg(test)]
mod tests;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
}

pub struct Removed {
    pub binaries: BTreeMap<String, PathBuf>,
    pub active: usize,
}
pub fn run(prefix: &Path, components: &[String], options: Options) -> Result<Removed> {
    let Options { yes, dry_run } = options;
    let mut lifecycle = services::Services::new(yes)?;
    remove_with(prefix, components, dry_run, &mut lifecycle, |_| Ok(()))
}
pub(crate) fn remove_with(
    prefix: &Path,
    components: &[String],
    dry_run: bool,
    lifecycle: &mut dyn Lifecycle,
    hook: impl FnMut(&str) -> Result<()>,
) -> Result<Removed> {
    let manager = prefix.join("lib/hive");
    if !manager.try_exists()? {
        return Err(format!("No managed installation at {}", manager.display()).into());
    }
    let _lock = publish::installation_lock(&manager)?;
    remove_locked(prefix, components, dry_run, lifecycle, hook)
}
pub(crate) fn remove_locked(
    prefix: &Path,
    components: &[String],
    dry_run: bool,
    lifecycle: &mut dyn Lifecycle,
    mut hook: impl FnMut(&str) -> Result<()>,
) -> Result<Removed> {
    let image = (!dry_run)
        .then(crate::publication::Image::capture)
        .transpose()?;
    let manager = prefix.join("lib/hive");
    publish::manager(prefix, &mut hook)?;
    if dry_run {
        if files::optional_metadata(&manager.join("pending-install.json"))?.is_some() {
            return Err(
                "Pending installation needs recovery before a read-only uninstall preview".into(),
            );
        }
    } else {
        lifecycle.recover(prefix, &manager)?;
    }
    lifecycle.preview_removal(prefix, components)?;
    let old = publish::current(&manager)?.ok_or("Installation has no current image")?;
    let mut installed = publish::load(&manager, Some(&old), &manifest::native_target()?)?;
    let old_links = publish::links(&installed.parts)?;
    if !publish::check_links(prefix, &old_links)?.is_empty() {
        return Err("Owned links are missing; refusing uninstall".into());
    }
    let binaries = retire_components(&mut installed, components, &old, &manager);
    let remaining = publish::links(&installed.parts)?;
    let removed: BTreeMap<_, _> = old_links
        .into_iter()
        .filter(|(name, _)| !remaining.contains_key(name))
        .collect();
    preview(prefix, &manager, &removed);
    let result = Removed {
        binaries,
        active: installed.parts.len(),
    };
    if dry_run || removed.is_empty() {
        return Ok(result);
    }
    tracing::info!(name: "install.uninstall_started", count = components.len());

    let generation = publish::stage_generation(
        &manager,
        &mut installed,
        publish::Generation {
            source: Path::new(""),
            selected: &[],
            previous: Some(&old),
            installer: image.as_ref().ok_or("Missing captured installer image")?,
        },
    )?;
    let change = Change {
        operation: Operation::Uninstall,
        prefix: prefix.into(),
        manager: manager.clone(),
        previous: Some(old),
        next: generation.strip_prefix(&manager)?.into(),
        components: components.to_vec(),
        added_links: BTreeMap::new(),
        removed_links: removed,
    };
    retire(&change, &generation, lifecycle, &mut hook)?;
    tracing::info!(name: "install.uninstall_finished", count = result.active, success = true);
    Ok(result)
}
fn retire(
    change: &Change,
    generation: &Path,
    lifecycle: &mut dyn Lifecycle,
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    if let Err(error) = lifecycle
        .prepare(change)
        .and_then(|()| execute(change, lifecycle, hook))
    {
        let cleanup = crate::publication::discard_unreferenced(&change.manager, generation);
        return Err(crate::publication::failed(
            error,
            "Uninstall cleanup failed",
            cleanup,
        ));
    }
    Ok(())
}

fn retire_components(
    installed: &mut manifest::Installed,
    components: &[String],
    old: &Path,
    manager: &Path,
) -> BTreeMap<String, PathBuf> {
    let mut binaries = BTreeMap::new();
    for component in components {
        if installed.parts.remove(component).is_some() {
            installed.retired.insert(component.clone(), old.to_owned());
        }
        // The retained server image needs no maintenance command.
        if let Some(pointer) = installed.retired.get(component)
            && let Some(binary) = manifest::maintenance_binary(component)
        {
            binaries.insert(
                component.clone(),
                manager.join(pointer).join("bin").join(binary),
            );
        }
    }
    binaries
}

fn preview(prefix: &Path, manager: &Path, removed: &BTreeMap<PathBuf, PathBuf>) {
    for name in removed.keys() {
        eprintln!("Remove owned entrypoint: {}", prefix.join(name).display());
    }
    eprintln!(
        "Retain configuration, state, native provider histories and Comb roots \
            unless --purge is explicit."
    );
    eprintln!(
        "Retain immutable images at {} (partial uninstall is not image pruning).",
        manager.join("generations").display()
    );
}

fn execute(
    change: &Change,
    lifecycle: &mut dyn Lifecycle,
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    if let Err(error) = (|| {
        lifecycle.stop()?;
        hook("before_replace")?;
        publish::replace(&change.manager, &change.next)?;
        hook("after_replace")?;
        publish::sync(&change.manager)?;
        services::reconcile_removed(&change.prefix, &change.removed_links, false)?;
        hook("after_links")?;
        Ok::<_, crate::Error>(())
    })() {
        // Once published, finish this uninstall on retry; never claim rollback.
        return Err(failed(change, lifecycle, error));
    }
    lifecycle.finish(true).map_err(|error| {
        crate::publication::context("Uninstall committed; cleanup needs recovery", error)
    })
}

fn failed(change: &Change, lifecycle: &mut dyn Lifecycle, error: crate::Error) -> crate::Error {
    let current = match publish::current(&change.manager) {
        Ok(current) => current,
        Err(secondary) => {
            return crate::publication::failed(
                error,
                "Uninstall publication status is uncertain",
                Err(secondary),
            );
        }
    };
    if current.as_ref() == Some(&change.next) {
        tracing::warn!(name: "install.uninstall_pending", decision = "finish_on_retry");
        return crate::publication::context(
            "Uninstall is committed but incomplete; retry to finish owned link/service removal",
            error,
        );
    }
    let restore = (|| {
        publish::sync(&change.manager)?;
        let restored = lifecycle.restore(false);
        let links = services::reconcile_removed(&change.prefix, &change.removed_links, true);
        crate::lifecycle::settle(restored, links)?;
        lifecycle.clear()
    })();
    crate::publication::failed(error, "Previous services need recovery", restore)
}
