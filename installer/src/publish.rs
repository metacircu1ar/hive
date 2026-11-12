//! One durable pointer publishes the complete command set; old images stay valid.
#[cfg(test)]
mod generation_tests;
mod install;
mod layout;
use crate::{
    Result,
    manifest::{self, Installed, Manifest},
    publication,
};
pub(crate) use layout::{OWNER, current, installation_lock, manager, sync};
use layout::{directory, prefix_directories, sync_parent};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
};

pub struct Report {
    pub generations: usize,
    pub bytes: u64,
}

pub(crate) fn load(manager: &Path, pointer: Option<&Path>, target: &str) -> Result<Installed> {
    crate::ownership::directory(manager, crate::ownership::Directory::Manager)?;
    crate::ownership::directory(
        &manager.join("generations"),
        crate::ownership::Directory::Inner,
    )?;
    let Some(pointer) = pointer else {
        return Ok(Installed {
            format: hive_input::compatibility::Exact,
            target: target.into(),
            retired: BTreeMap::new(),
            parts: BTreeMap::new(),
            installer_notices: manifest::File {
                sha256: String::new(),
                bytes: 0,
                mode: 0o644,
            },
            installer: manifest::File {
                sha256: String::new(),
                bytes: 0,
                mode: 0o755,
            },
        });
    };
    let root = manager.join(pointer);
    crate::ownership::directory(&root, crate::ownership::Directory::Inner)?;
    let installed: Installed = manifest::installed(&crate::ownership::record(
        &root.join("installation.json"),
        4 * 1024 * 1024,
    )?)?;
    // Hive, Comb and Comb's bundled llama-server are the only components.
    if installed.target != target || installed.parts.len() > 3 || installed.retired.len() > 3 {
        return Err("Unsupported installation manifest".into());
    }
    for (component, part) in &installed.parts {
        part.validate(target)?;
        if component != part.component()? {
            return Err("Invalid component ownership".into());
        }
        part.check_files(&root)?;
    }
    check_installer(&root, &installed.installer)?;
    check_notices(&root, &installed.installer_notices)?;
    for (component, pointer) in &installed.retired {
        retired_part(manager, pointer, component, target)?;
    }
    Ok(installed)
}
pub(crate) fn links(parts: &BTreeMap<String, Manifest>) -> Result<BTreeMap<PathBuf, PathBuf>> {
    let mut links = BTreeMap::new();
    if !parts.is_empty() {
        links.insert(
            "bin/hive-install".into(),
            "../lib/hive/current/bin/hive-install".into(),
        );
    }
    for (component, part) in parts {
        for binary in part.binaries()? {
            links.insert(
                PathBuf::from(format!("bin/{binary}")),
                PathBuf::from(format!("../lib/hive/current/bin/{binary}")),
            );
        }
        links.insert(
            PathBuf::from(format!("share/{component}")),
            PathBuf::from(format!("../lib/hive/current/share/{component}")),
        );
    }
    Ok(links)
}
pub(crate) fn check_links(
    prefix: &Path,
    expected: &BTreeMap<PathBuf, PathBuf>,
) -> Result<Vec<PathBuf>> {
    expected
        .iter()
        .map(|(name, target)| {
            let path = prefix.join(name);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_symlink() && fs::read_link(&path)? == *target => Ok(None),
                Ok(_) => Err(format!(
                    "Refusing to overwrite unowned command or docs: {}",
                    path.display()
                )
                .into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(Some(name.clone()))
                }
                Err(error) => Err(error.into()),
            }
        })
        .filter_map(Result::transpose)
        .collect()
}
fn populate(
    root: &Path,
    source: &Path,
    parts: &BTreeMap<String, Manifest>,
    selected: &[Manifest],
    old: Option<&Path>,
) -> Result<()> {
    // Validate both sets before copying any payload: two malformed part names
    // must never become equal by discarding their component errors.
    let selected_components = selected
        .iter()
        .map(Manifest::component)
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    let components = parts
        .values()
        .map(|part| Ok((part, part.component()?)))
        .collect::<Result<Vec<_>>>()?;
    for (part, component) in components {
        let replacing = selected_components.contains(component);
        let origin = if replacing {
            source.join(&part.part)
        } else {
            old.ok_or("Missing retained generation")?.to_owned()
        };
        for (name, file) in &part.files {
            let destination = root.join(name);
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o755)
                .create(destination.parent().ok_or("Missing payload parent")?)?;
            if replacing {
                publication::copy(&origin.join(name), &destination, file.mode)?;
            } else {
                // Never chmod, truncate or modify a retained shared inode.
                fs::hard_link(origin.join(name), &destination)?;
            }
            sync(&destination)?;
        }
        part.check_files(root)?;
        part.check_versions(root)?;
    }
    Ok(())
}
fn sync_tree(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if fs::symlink_metadata(&path)?.is_dir() {
            sync_tree(&path)?;
        }
    }
    sync(root)
}

pub(crate) fn replace(manager: &Path, target: &Path) -> Result<()> {
    replace_with(manager, target, &atomic_fs::DefaultBarriers)
}
pub(crate) fn replace_with(
    manager: &Path,
    target: &Path,
    barriers: &impl atomic_fs::Barriers,
) -> Result<()> {
    let directory = tempfile::Builder::new()
        .prefix(".pointer-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(manager)?;
    let directory = publication::Owned::new(directory)?;
    let parent = fs::File::open(directory.path())?;
    let link = directory.path().join("next");
    let mut published = false;
    let result = (|| {
        symlink(target, &link)?;
        barriers.sync_directory(&parent)?;
        fs::rename(link, manager.join("current"))?;
        published = true;
        // Both source and destination directory barriers are required. The
        // transaction syncs manager before exposing added links or restoring service.
        barriers.sync_directory(&parent)
    })();
    directory.finish(result.map_err(|source| atomic_fs::Error::new(source, published).into()))
}
pub(crate) fn rollback(
    manager: &Path,
    old: Option<&Path>,
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    hook("rollback")?;
    if let Some(target) = old {
        if current(manager)?.as_deref() != Some(target) {
            replace(manager, target)?;
        }
    } else if hive_setup_support::files::optional_metadata(&manager.join("current"))?.is_some() {
        fs::remove_file(manager.join("current"))?;
    }
    hook("rollback_sync")?;
    sync(manager)?;
    if current(manager)?.as_deref() != old {
        return Err("Rollback pointer could not be confirmed".into());
    }
    tracing::info!(name: "install.rollback_finished", confirmed = true);
    Ok(())
}
fn remove_links(
    prefix: &Path,
    names: &[PathBuf],
    expected: &BTreeMap<PathBuf, PathBuf>,
) -> Result<()> {
    for name in names {
        let path = prefix.join(name);
        if fs::read_link(&path)? != expected[name] {
            return Err("New command link changed during rollback".into());
        }
        fs::remove_file(path)?;
    }
    sync(&prefix.join("bin"))?;
    sync(&prefix.join("share"))?;
    Ok(())
}

type Links = BTreeMap<PathBuf, PathBuf>;
fn prepare_links(
    prefix: &Path,
    installed: &mut Installed,
    selected: &[Manifest],
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<(Links, Vec<PathBuf>)> {
    let previously_owned = links(&installed.parts)?;
    directory(&prefix.join("bin"), hook)?;
    directory(&prefix.join("share"), hook)?;
    if !check_links(prefix, &previously_owned)?.is_empty() {
        return Err("Owned command links are missing; refusing partial installation".into());
    }
    for part in selected {
        installed.retired.remove(part.component()?);
        installed
            .parts
            .insert(part.component()?.into(), part.clone());
    }
    let expected = links(&installed.parts)?;
    for name in expected.keys() {
        if !previously_owned.contains_key(name)
            && hive_setup_support::files::optional_metadata(&prefix.join(name))?.is_some()
        {
            return Err(format!(
                "Unowned or interrupted-install link requires inspection: {}",
                prefix.join(name).display()
            )
            .into());
        }
    }
    let missing = check_links(prefix, &expected)?;
    Ok((expected, missing))
}

#[cfg(test)]
pub fn install(
    prefix: &Path,
    source: &Path,
    selected: &[Manifest],
    hook: impl FnMut(&str) -> Result<()>,
) -> Result<Option<Report>> {
    install_with(prefix, source, selected, &mut crate::lifecycle::Inert, hook)
}

pub fn install_with(
    prefix: &Path,
    source: &Path,
    selected: &[Manifest],
    lifecycle: &mut dyn crate::lifecycle::Lifecycle,
    mut hook: impl FnMut(&str) -> Result<()>,
) -> Result<Option<Report>> {
    let target = validate_sources(prefix, source, selected)?;
    let installer = publication::Image::capture()?;
    let mut observed = |phase: &str| {
        crate::diagnostics::phase(phase);
        hook(phase)
    };
    prefix_directories(prefix, &mut observed)?;
    directory(&prefix.join("lib"), &mut observed)?;
    let owner = std::sync::Arc::new(installation_lock(&prefix.join("lib/hive"))?);
    crate::purge::fence_install(prefix)?;
    let manager = manager(prefix, &mut observed)?;
    crate::diagnostics::owned(prefix, owner, || {
        install::Installation {
            prefix,
            source,
            selected,
            manager: &manager,
            target: &target,
            installer: &installer,
        }
        .run(lifecycle, &mut observed)
    })
}

fn validate_sources(prefix: &Path, source: &Path, selected: &[Manifest]) -> Result<String> {
    crate::options::input::validate_prefix(prefix)?;
    layout::inspect_existing(prefix)?;
    let target = manifest::native_target()?;
    // Even direct native-helper use validates all parts before prefix creation.
    for part in selected {
        part.validate(&target)?;
        part.check_files(&source.join(&part.part))?;
        part.check_versions(&source.join(&part.part))?;
    }
    Ok(target)
}
struct Activation<'a> {
    prefix: &'a Path,
    manager: &'a Path,
    pointer: &'a Path,
    expected: &'a Links,
    missing: &'a [PathBuf],
    created: Vec<PathBuf>,
}

impl Activation<'_> {
    fn run(&mut self, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
        hook("before_replace")?;
        replace(self.manager, self.pointer)?;
        hook("after_replace")?;
        hook("before_sync")?;
        sync(self.manager)?;
        hook("after_sync")?;
        for name in self.missing {
            symlink(&self.expected[name], self.prefix.join(name))?;
            self.created.push(name.clone());
            hook("link_created")?;
        }
        sync(&self.prefix.join("bin"))?;
        sync(&self.prefix.join("share"))?;
        hook("after_links")?;
        Ok(())
    }
}
pub(crate) fn usage(root: &Path) -> Result<Report> {
    fn visit(path: &Path, seen: &mut BTreeSet<(u64, u64)>, bytes: &mut u64) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                visit(&path, seen, bytes)?;
            } else if meta.is_file() && seen.insert((meta.dev(), meta.ino())) {
                *bytes += meta.len();
            }
        }
        Ok(())
    }
    let generations = fs::read_dir(root)?.try_fold(0_usize, |count, entry| {
        let entry = entry?;
        Ok::<_, std::io::Error>(
            count
                + usize::from(
                    entry.file_type()?.is_dir()
                        && entry.file_name().as_encoded_bytes().starts_with(b"g-"),
                ),
        )
    })?;
    let mut bytes = 0;
    visit(root, &mut BTreeSet::new(), &mut bytes)?;
    Ok(Report { generations, bytes })
}

fn check_installer(root: &Path, expected: &manifest::File) -> Result<()> {
    let path = root.join("bin/hive-install");
    if !fs::symlink_metadata(root.join("bin"))?.is_dir() {
        return Err("Installer parent is not a real directory".into());
    }
    if expected.mode != 0o755 {
        return Err("Owned installation helper was modified".into());
    }
    manifest::check_payload(&path, expected)
}

pub(crate) struct Generation<'a> {
    pub source: &'a Path,
    pub selected: &'a [Manifest],
    pub previous: Option<&'a Path>,
    pub installer: &'a publication::Image,
}
pub(crate) fn stage_generation(
    manager: &Path,
    installed: &mut Installed,
    input: Generation<'_>,
) -> Result<PathBuf> {
    let stage = publication::Owned::new(
        tempfile::Builder::new()
            .prefix(".preparing-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(manager.join("generations"))?,
    )?;
    let prepared = (|| {
        populate(
            stage.path(),
            input.source,
            &installed.parts,
            input.selected,
            input.previous.map(|p| manager.join(p)).as_deref(),
        )?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(stage.path().join("bin"))?;
        (installed.installer, installed.installer_notices) = input.installer.copy(stage.path())?;
        publication::write(
            &stage.path().join("installation.json"),
            &serde_json::to_vec_pretty(installed)?,
        )?;
        sync_tree(stage.path())
    })();
    if let Err(error) = prepared {
        return stage.finish(Err(error));
    }
    let name = stage
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix(".preparing-"))
        .ok_or("Invalid private generation preparation name")?;
    let target = manager.join("generations").join(format!("g-{name}"));
    let path = stage.keep()?;
    hive_setup_support::removal::rename_noreplace(&path, &target)
        .map_err(|cause| atomic_fs::Error::new(cause, false).with_retained(&path))?;
    sync(&manager.join("generations")).map_err(|cause| {
        atomic_fs::Error::new(cause.into_io_other(), true).with_retained(&target)
    })?;
    Ok(target)
}
fn check_notices(root: &Path, expected: &manifest::File) -> Result<()> {
    let path = root.join("bin/hive-install.LICENSES");
    if expected.mode != 0o644 {
        return Err("Owned installer notices were modified".into());
    }
    manifest::check_payload(&path, expected)
}

pub(crate) fn retired_part(
    manager: &Path,
    pointer: &Path,
    component: &str,
    target: &str,
) -> Result<Manifest> {
    if pointer.components().count() != 2
        || !pointer.starts_with("generations")
        || pointer
            .file_name()
            .is_none_or(|p| !p.to_string_lossy().starts_with("g-"))
        || !pointer.to_str().is_some_and(manifest::safe_path)
    {
        return Err("Invalid retired generation pointer".into());
    }
    let root = manager.join(pointer);
    crate::ownership::directory(manager, crate::ownership::Directory::Manager)?;
    crate::ownership::directory(
        &manager.join("generations"),
        crate::ownership::Directory::Inner,
    )?;
    crate::ownership::directory(&root, crate::ownership::Directory::Inner)?;
    let image: Installed = manifest::installed(&crate::ownership::record(
        &root.join("installation.json"),
        4 * 1024 * 1024,
    )?)?;
    if image.target != target {
        return Err("Retired image target mismatch".into());
    }
    let part = image
        .parts
        .get(component)
        .ok_or("Retired image lacks the removed component")?;
    part.validate(target)?;
    if part.component()? != component {
        return Err("Retired component ownership mismatch".into());
    }
    part.check_files(&root)?;
    Ok(part.clone())
}

#[cfg(test)]
mod component_tests {
    use super::*;

    #[test]
    fn invalid_selected_or_retained_component_never_copies_a_payload() {
        let root = tempfile::tempdir().unwrap();
        let valid = crate::tests::fixture(root.path(), "hive");
        let mut invalid = valid.clone();
        invalid.part = "unknown".into();
        // If comparison discards both errors, this tree would be copied before
        // check_files notices that the component name is invalid.
        fs::rename(root.path().join("hive"), root.path().join("unknown")).unwrap();
        for (parts, selected) in [
            (vec![invalid.clone()], vec![invalid.clone()]),
            (vec![valid.clone()], vec![invalid.clone()]),
            (vec![valid.clone(), invalid], vec![valid]),
        ] {
            let parts = parts
                .into_iter()
                .enumerate()
                .map(|(i, part)| (i.to_string(), part))
                .collect();
            let destination = root.path().join("destination");
            let result = populate(&destination, root.path(), &parts, &selected, None);
            assert_eq!(result.unwrap_err().to_string(), "Unknown release part");
            assert!(!destination.exists());
        }
    }
}
