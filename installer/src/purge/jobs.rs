use super::{Args, Job, Plan, Receipt, Result, Scope, manifest, publish, removal};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) fn prepare(
    prefix: &Path,
    args: &Args,
    binaries: &BTreeMap<String, PathBuf>,
) -> Result<(Vec<Job>, Scope)> {
    let mut jobs = vec![];
    let mut scope = Scope::default();
    if args.components().iter().any(|s| s == "hive")
        && let Some(binary) = binaries.get("hive")
    {
        let mut command = vec![];
        command.extend(path_args("--config", args.config.as_deref())?);
        command.extend(path_args("--state-dir", args.state_dir.as_deref())?);
        command.push("setup-maintenance".into());
        command.extend(scope_args(
            args.comb_root.as_deref(),
            args.bridge_file.as_deref(),
        )?);
        let plan = call(binary, &command)?;
        scope = plan.scope.clone();
        jobs.push(job(prefix, "hive", binary, plan));
    }
    let detach = args.components().iter().any(|s| s == "comb");
    if detach || scope.bridge.is_some() || args.bridge_file.is_some() {
        let binary = binaries.get("comb").ok_or(
            "Comb maintenance binary is required to verify and revoke this bridge; \
            retain it or install Comb before purge",
        )?;
        let mut command = vec!["setup-maintenance".into()];
        command.extend(scope_args(
            args.comb_root.as_deref().or(scope.root.as_deref()),
            args.bridge_file.as_deref().or(scope.bridge.as_deref()),
        )?);
        if detach {
            command.push("--detach-comb".into());
        }
        command.extend(path_args(
            "--purge-comb-root",
            args.purge_comb_root.as_deref(),
        )?);
        let plan = call(binary, &command)?;
        scope.root.clone_from(&plan.scope.root);
        scope.bridge.clone_from(&plan.scope.bridge);
        let job = job(prefix, "comb", binary, plan);
        // A retained, possibly serving Comb must revoke before Hive config goes away.
        if detach {
            jobs.push(job);
        } else {
            jobs.insert(0, job);
        }
    }
    for job in &jobs {
        if hive_setup_support::files::optional_metadata(&job.receipt)?.is_some() {
            return Err(
                "Orphan native purge receipt exists; inspect it before admitting a new purge"
                    .into(),
            );
        }
    }
    Ok((jobs, scope))
}
fn job(prefix: &Path, component: &str, binary: &Path, plan: Plan<serde_json::Value>) -> Job {
    Job {
        component: component.into(),
        binary: binary.into(),
        receipt: prefix.join(format!("lib/.hive-purge-{component}.json")),
        plan,
        done: false,
    }
}
fn scope_args(root: Option<&Path>, bridge: Option<&Path>) -> Result<impl Iterator<Item = String>> {
    Ok(path_args("--knowledge-root", root)?.chain(path_args("--bridge-file", bridge)?))
}
fn path_args(flag: &str, path: Option<&Path>) -> Result<impl Iterator<Item = String>> {
    let pair = path
        .map(|path| -> Result<[String; 2]> {
            Ok([flag.into(), hive_setup_support::service::path_text(path)?])
        })
        .transpose()?;
    Ok(pair.into_iter().flatten())
}
fn call(binary: &Path, args: &[String]) -> Result<Plan<serde_json::Value>> {
    let output =
        crate::native::progress(binary.to_str().ok_or("Invalid native program path")?, args)?;
    if output.code != 0 {
        return Err("Native maintenance failed; inspect the retained purge receipt.".into());
    }
    Ok(hive_input::compatibility::record::decode(
        output.stdout.as_bytes(),
        &hive_setup_support::compatibility::MAINTENANCE,
    )?)
}
pub(super) fn apply(job: &Job) -> Result<()> {
    let result = call(
        &job.binary,
        &[
            "setup-maintenance".into(),
            "--apply".into(),
            hive_setup_support::service::path_text(&job.receipt)?,
        ],
    )?;
    if result.product != job.plan.product
        || result.scope != job.plan.scope
        || result.removals.iter().any(|a| !a.done)
    {
        return Err("Native cleanup did not confirm its exact admitted scope".into());
    }
    Ok(())
}
pub(super) fn verify_binary(manager: &Path, job: &Job) -> Result<()> {
    let relative = job.binary.strip_prefix(manager)?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 4
        || parts[2].as_os_str() != "bin"
        || manifest::maintenance_binary(&job.component)
            .is_none_or(|binary| parts[3].as_os_str() != binary)
    {
        return Err("Invalid retained maintenance executable".into());
    }
    let pointer = PathBuf::from(parts[0].as_os_str()).join(parts[1].as_os_str());
    publish::retired_part(
        manager,
        &pointer,
        &job.component,
        &manifest::native_target()?,
    )?;
    Ok(())
}
pub(super) fn check_options(args: &Args, receipt: &Receipt) -> Result<()> {
    if args.components() != receipt.components {
        return Err("Purge retry must select the original components".into());
    }
    for (flag, expected) in [
        (&args.config, &receipt.scope.config),
        (&args.state_dir, &receipt.scope.state),
        (&args.comb_root, &receipt.scope.root),
        (&args.bridge_file, &receipt.scope.bridge),
        (&args.purge_comb_root, &receipt.purge_root),
    ] {
        if let Some(path) = flag
            && Some(removal::safe_path(path)?) != *expected
        {
            return Err(
                "Purge flags conflict with the durable original scope; use its recorded paths"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    #[test]
    fn native_plan_and_apply_replies_classify_versions_and_refuse_positional_records() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("maintenance");
        for (body, newer) in [
            (r#"{"format":2,"future_field":true}"#, true),
            (
                r#"[1,"Hive",{"config":null,"state":null,"root":null,"bridge":null},[],[],null]"#,
                false,
            ),
        ] {
            let script = format!("#!/bin/sh\nprintf '%s\\n' '{body}'\n");
            atomic_fs::Publisher::new(atomic_fs::Options::new().with_mode(0o755))
                .write(&binary, script.as_bytes())
                .unwrap();
            for arguments in [
                vec!["setup-maintenance".into()],
                vec![
                    "setup-maintenance".into(),
                    "--apply".into(),
                    "receipt".into(),
                ],
            ] {
                let error = call(&binary, &arguments).err().unwrap();
                let cause = error
                    .downcast_ref::<hive_input::compatibility::record::Error>()
                    .unwrap();
                assert_eq!(cause.newer(), newer);
                assert_eq!(std::fs::read_to_string(&binary).unwrap(), script);
            }
        }
    }
    #[test]
    fn optional_scope_arguments_preserve_order_and_validate_before_iteration() {
        for root in [None, Some(Path::new("root with spaces"))] {
            for bridge in [None, Some(Path::new("bridge-file"))] {
                let actual: Vec<_> = scope_args(root, bridge).unwrap().collect();
                let mut expected = Vec::new();
                if root.is_some() {
                    expected.extend(["--knowledge-root", "root with spaces"]);
                }
                if bridge.is_some() {
                    expected.extend(["--bridge-file", "bridge-file"]);
                }
                assert_eq!(actual, expected);
            }
        }
        let invalid = PathBuf::from(std::ffi::OsString::from_vec(b"invalid-\xff".to_vec()));
        assert!(scope_args(Some(&invalid), Some(Path::new("bridge"))).is_err());
        assert!(scope_args(Some(Path::new("root")), Some(&invalid)).is_err());
        let actual: Vec<_> = path_args("--config", Some(Path::new("config file")))
            .unwrap()
            .collect();
        assert_eq!(actual, ["--config", "config file"]);
        assert_eq!(path_args("--config", None).unwrap().count(), 0);
    }

    #[test]
    fn deletion_paths_never_become_a_lossy_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join(std::ffi::OsString::from_vec(b"scope-\xff".to_vec()));
        let sibling = temp.path().join("scope-\u{fffd}");
        // APFS rejects invalid UTF-8 filenames; Linux permits them.
        #[cfg(target_os = "linux")]
        std::fs::write(&path, b"original").unwrap();
        std::fs::write(&sibling, b"unrelated").unwrap();
        assert!(path_args("--bridge-file", Some(&path)).is_err());
        #[cfg(target_os = "linux")]
        assert_eq!(std::fs::read(path).unwrap(), b"original");
        assert_eq!(std::fs::read(sibling).unwrap(), b"unrelated");
    }

    #[test]
    fn retired_components_use_their_cli_and_reject_other_owned_binaries() -> Result<()> {
        let root = tempfile::tempdir()?;
        let prefix = root.path().join("prefix");
        let parts = [
            crate::tests::fixture(root.path(), "hive"),
            crate::tests::fixture(root.path(), "comb"),
        ];
        publish::install(&prefix, root.path(), &parts, |_| Ok(()))?;
        let removed = crate::uninstall::remove_with(
            &prefix,
            &["hive".into(), "comb".into()],
            false,
            &mut crate::lifecycle::Inert,
            |_| Ok(()),
        )?;
        assert_eq!(removed.binaries.len(), 2);
        let manager = prefix.join("lib/hive");
        for (component, binary) in removed.binaries {
            let mut retained = job(
                &prefix,
                &component,
                &binary,
                Plan {
                    format: hive_input::compatibility::Exact,
                    product: component.clone(),
                    scope: Scope::default(),
                    retained: vec![],
                    removals: vec![],
                    details: serde_json::Value::Null,
                },
            );
            verify_binary(&manager, &retained)?;
            if component == "hive" {
                for other in ["hive", "hive-daemon"] {
                    retained.binary.set_file_name(other);
                    assert!(verify_binary(&manager, &retained).is_err());
                }
            }
        }
        Ok(())
    }
}
