use crate::{Result, manifest, ownership, publish};
use hive_setup_support::{
    files,
    removal::{Action, Identity},
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) fn remove(action: &mut Action) -> Result<()> {
    remove_with_scan(
        action,
        hive_setup_support::process_images::ensure_unused,
        hive_setup_support::removal::rename_noreplace,
    )
}
fn remove_with_scan(
    action: &mut Action,
    mut scan: impl FnMut(&[&Path]) -> std::io::Result<()>,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    action.validate()?;
    if action.done {
        action.remove()?;
        return Ok(());
    }
    let original = &action.object.path;
    let mut renamed = None;
    if files::optional_metadata(original)?.is_some() {
        action.object.check()?;
        validate(original)?;
        scan(&[original, &action.quarantine])?;
        if files::optional_metadata(&action.quarantine)?.is_some() {
            return Err("Image quarantine already exists".into());
        }
        let current = saved_identity(action, original)?;
        rename(original, &action.quarantine)?;
        renamed = Some(current);
        files::sync_parent(original)?;
    } else if Identity::read(&action.quarantine)?.is_none() {
        // The caller durably recorded image admission before the first rename.
        action.remove()?;
        return Ok(());
    } else {
        saved_identity(action, &action.quarantine)?;
    }
    if let Err(error) = scan(&[original, &action.quarantine]) {
        let restore = (|| -> Result<()> {
            if let Some(current) = &renamed {
                if files::optional_metadata(original)?.is_some()
                    || Identity::read(&action.quarantine)?.as_ref() != Some(current)
                {
                    return Err("Image restore identity changed; retain quarantine".into());
                }
                rename(&action.quarantine, original)?;
                files::sync_parent(original)?;
            }
            Ok(())
        })();
        return Err(crate::publication::failed(
            error.into(),
            "Image restore failed",
            restore,
        ));
    }
    action.remove()?;
    Ok(())
}
fn saved_identity(action: &Action, path: &Path) -> Result<Identity> {
    const CHANGED: &str = "Image manager/quarantine identity changed; refusing deletion";
    let current = Identity::read(path)?.ok_or(CHANGED)?;
    if !action
        .object
        .identity
        .as_ref()
        .map(|saved| saved.matches_saved(&current, path))
        .transpose()?
        .unwrap_or(false)
    {
        return Err(CHANGED.into());
    }
    Ok(current)
}
pub(super) fn validate(manager: &Path) -> Result<()> {
    let device = ownership::directory(manager, ownership::Directory::Manager)?.dev();
    let generations = manager.join("generations");
    if ownership::directory(&generations, ownership::Directory::Inner)?.dev() != device {
        return Err("Generations directory is on another filesystem".into());
    }
    ownership::owner_record(&manager.join("owner"), publish::OWNER)?;
    let expected = BTreeSet::from(["owner".into(), "current".into(), "generations".into()]);
    let entries: BTreeSet<String> = fs::read_dir(manager)?
        .map(|e| {
            Ok(e?
                .file_name()
                .into_string()
                .map_err(|_| "Invalid manager filename")?)
        })
        .collect::<Result<_>>()?;
    if entries != expected {
        return Err(concat!(
            "Unknown installation manager entries; image purge refused. ",
            "Retain the manager and inspect recovery evidence before removing any preparation."
        )
        .into());
    }
    let current = publish::current(manager)?.ok_or("Missing current generation")?;
    let target = manifest::native_target()?;
    if !publish::load(manager, Some(&current), &target)?
        .parts
        .is_empty()
    {
        return Err("Active components remain; image purge refused".into());
    }
    for entry in fs::read_dir(manager.join("generations"))? {
        let path = entry?.path();
        validate_generation(manager, &path, &target, device)?;
    }
    Ok(())
}
fn validate_generation(manager: &Path, path: &Path, target: &str, device: u64) -> Result<()> {
    if !path
        .file_name()
        .is_some_and(|s| s.to_string_lossy().starts_with("g-"))
        || !fs::symlink_metadata(path)?.is_dir()
    {
        return Err(concat!(
            "Unrecognized generation directory; image purge refused. ",
            "Retain unknown preparations and inspect recovery evidence before manual cleanup."
        )
        .into());
    }
    let installed = publish::load(manager, Some(path.strip_prefix(manager)?), target)?;
    let mut expected = BTreeSet::from([
        PathBuf::from("installation.json"),
        PathBuf::from("bin/hive-install"),
        PathBuf::from("bin/hive-install.LICENSES"),
    ]);
    for part in installed.parts.values() {
        expected.extend(part.files.keys().map(PathBuf::from));
    }
    let directories: BTreeSet<_> = expected
        .iter()
        .flat_map(|p| p.ancestors().skip(1).map(Path::to_path_buf))
        .collect();
    let mut observed = BTreeSet::new();
    inventory(path, path, device, &directories, &mut observed)?;
    if expected != observed {
        return Err(format!("Unknown files in retained generation {}", path.display()).into());
    }
    Ok(())
}

fn inventory(
    root: &Path,
    path: &Path,
    device: u64,
    directories: &BTreeSet<PathBuf>,
    observed: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    Identity::read(path)?.ok_or("Generation path disappeared")?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.dev() != device {
        return Err("Generation contains another filesystem".into());
    }
    if metadata.is_dir() {
        if !directories.contains(path.strip_prefix(root)?) {
            return Err("Unknown directory in retained generation".into());
        }
        for entry in fs::read_dir(path)? {
            inventory(root, &entry?.path(), device, directories, observed)?;
        }
    } else {
        observed.insert(path.strip_prefix(root)?.into());
    }
    Ok(())
}

#[cfg(test)]
mod collisions;
#[cfg(test)]
mod renumbering;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_image_purge_removes_all_generations_and_retains_external_lock() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("prefix");
        let part = crate::tests::fixture(temp.path(), "hive");
        publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
        let diagnostic = prefix.join("lib/hive-install-logs/install.jsonl");
        assert!(
            fs::read_to_string(&diagnostic)
                .unwrap()
                .contains("install.finished")
        );
        crate::uninstall::remove_with(
            &prefix,
            &["hive".into()],
            false,
            &mut crate::lifecycle::Inert,
            |_| Ok(()),
        )
        .unwrap();
        let manager = prefix.join("lib/hive");
        let mut action = Action::capture(&manager, "images").unwrap();
        validate(&manager).unwrap();
        remove(&mut action).unwrap();
        assert!(!manager.exists());
        assert!(prefix.join("lib/.hive-install.lock").is_file());
        assert!(diagnostic.is_file());
        remove(&mut action).unwrap();
        assert!(!manager.exists());
        assert!(diagnostic.is_file());
    }
    #[test]
    fn unknown_generation_files_refuse_image_purge() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("prefix");
        let part = crate::tests::fixture(temp.path(), "hive");
        publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
        crate::uninstall::remove_with(
            &prefix,
            &["hive".into()],
            false,
            &mut crate::lifecycle::Inert,
            |_| Ok(()),
        )
        .unwrap();
        let manager = prefix.join("lib/hive");
        let generation = manager.join(publish::current(&manager).unwrap().unwrap());
        fs::create_dir(generation.join("personal-empty-directory")).unwrap();
        assert!(validate(&manager).is_err());
        fs::remove_dir(generation.join("personal-empty-directory")).unwrap();
        fs::write(generation.join("personal"), b"keep").unwrap();
        let mut action = Action::capture(&manager, "images").unwrap();
        assert!(remove(&mut action).is_err());
        assert_eq!(fs::read(generation.join("personal")).unwrap(), b"keep");
    }
    #[test]
    fn failed_post_rename_scan_keeps_primary_when_restore_also_fails() {
        for obstruct in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let prefix = temp.path().join("prefix");
            let part = crate::tests::fixture(temp.path(), "hive");
            publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
            crate::uninstall::remove_with(
                &prefix,
                &["hive".into()],
                false,
                &mut crate::lifecycle::Inert,
                |_| Ok(()),
            )
            .unwrap();
            let manager = prefix.join("lib/hive");
            let mut action = Action::capture(&manager, "images").unwrap();
            let mut calls = 0;
            let error = remove_with_scan(
                &mut action,
                |_| {
                    calls += 1;
                    if calls == 1 {
                        return Ok(());
                    }
                    if obstruct {
                        fs::create_dir(&manager)?;
                        fs::write(manager.join("foreign"), b"keep")?;
                    }
                    Err(std::io::Error::from_raw_os_error(
                        rustix::io::Errno::NOSPC.raw_os_error(),
                    ))
                },
                hive_setup_support::removal::rename_noreplace,
            )
            .unwrap_err();
            let mut cause: &(dyn std::error::Error + 'static) = error.as_ref();
            while let Some(source) = cause.source() {
                cause = source;
            }
            assert_eq!(
                cause
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(rustix::io::Errno::NOSPC.raw_os_error())
            );
            assert_eq!(action.quarantine.exists(), obstruct);
            assert!(!action.done);
            if obstruct {
                assert_eq!(fs::read(manager.join("foreign")).unwrap(), b"keep");
            } else {
                validate(&manager).unwrap();
            }
        }
    }
    #[test]
    fn partial_quarantine_survives_scan_refusal_then_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let manager = temp.path().join("manager");
        fs::create_dir(&manager).unwrap();
        fs::write(manager.join("remaining-image"), b"partial cleanup").unwrap();
        let mut action = Action::capture(&manager, "images").unwrap();
        fs::rename(&manager, &action.quarantine).unwrap();
        assert!(
            remove_with_scan(
                &mut action,
                |_| Err(std::io::Error::other("live process")),
                hive_setup_support::removal::rename_noreplace,
            )
            .is_err()
        );
        assert!(!manager.exists());
        assert!(action.quarantine.join("remaining-image").exists());
        remove_with_scan(
            &mut action,
            |_| Ok(()),
            hive_setup_support::removal::rename_noreplace,
        )
        .unwrap();
        assert!(action.done);
        assert!(!action.quarantine.exists());
    }
}
