use super::*;
const ROOT: &str = "HIVE_INSTALL_GENERATION_TEST_ROOT";

#[test]
#[ignore = "owned process fixture, terminated before a generation is populated"]
fn preparation_death_fixture() {
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let prefix = root.join("prefix");
    let part = crate::tests::fixture(&root, "hive");
    let generations = prefix.join("lib/hive/generations");
    publication::with_creation_hook(
        move |path| {
            if path.parent() == Some(generations.as_path()) {
                std::process::exit(73);
            }
            Ok(())
        },
        || install(&prefix, &root, &[part], |_| Ok(())),
    )
    .unwrap();
    panic!("generation preparation death point was not reached");
}

#[test]
fn process_death_never_exposes_partial_generation_and_retry_preserves_unknown_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let part = crate::tests::fixture(root, "hive");
    let executable = std::env::current_exe().unwrap();
    let mut command = std::process::Command::new(executable);
    command.env(ROOT, root).args([
        "--exact",
        "publish::generation_tests::preparation_death_fixture",
        "--ignored",
    ]);
    let result = hive_setup_support::runner::capture_command(
        std::process::Command::spawn,
        &mut command,
        std::time::Duration::from_secs(30),
        &|| false,
    )
    .unwrap();
    assert_eq!(result.code, 73, "{}", result.stdout);
    let manager = root.join("prefix/lib/hive");
    let entries: Vec<_> = fs::read_dir(manager.join("generations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    let preparation = &entries[0];
    assert!(
        preparation
            .file_name()
            .unwrap()
            .as_encoded_bytes()
            .starts_with(b".preparing-")
    );
    let identity = atomic_fs::DirectoryIdentity::of(preparation).unwrap();
    assert!(!preparation.join("installation.json").exists());
    let report = install(&root.join("prefix"), root, &[part], |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(report.generations, 1);
    assert_eq!(
        atomic_fs::DirectoryIdentity::of(preparation).unwrap(),
        identity
    );
    assert!(current(&manager).unwrap().is_some());
}
