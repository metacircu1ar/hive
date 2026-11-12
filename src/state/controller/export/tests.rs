use super::*;
use atomic_fs::{Barriers, Step};
use std::{error::Error as _, fs::File, os::unix::fs::symlink};

struct Fail(Step);
impl Barriers for Fail {
    fn observe(&self, step: Step) -> io::Result<()> {
        if step == self.0 {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
}

fn fixture(root: &Path) -> (State, rusqlite::Connection) {
    let old = State::new(root.join("old")).unwrap();
    let (_, db) = super::super::tests::wal_export_fixture(&old);
    let fresh = old
        .replacement_root(&root.join("fresh"), crate::config::Provider::Codex)
        .unwrap();
    (fresh, db)
}

#[test]
fn export_admits_selected_root_and_refuses_symlinked_retired_source() {
    let temporary = tempfile::tempdir().unwrap();
    let (fresh, _db) = fixture(temporary.path());
    let output = temporary.path().join("export.ndjson");
    let marker = fresh.root.join("format");
    let original = fs::read(&marker).unwrap();
    for bytes in [br#"{"format":2,"extra":true}"#.as_slice(), b"invalid"] {
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&marker, bytes)
            .unwrap();
        assert!(fresh.export_retired(&output).is_err());
        assert!(!output.exists());
        assert_eq!(fs::read(&marker).unwrap(), bytes);
    }
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&marker, original)
        .unwrap();
    let source = fresh.retired_controller().unwrap().unwrap();
    let moved = source.with_file_name("retained-source");
    fs::rename(&source, &moved).unwrap();
    symlink(&moved, &source).unwrap();
    assert!(fresh.export_retired(&output).is_err());
    assert!(!output.exists());
    assert_eq!(fs::read_link(&source).unwrap(), moved);
}

#[test]
fn duplicate_evidence_keys_become_unavailable_without_stopping_later_records() {
    let temporary = tempfile::tempdir().unwrap();
    let bad = temporary.path().join("bad.json");
    let good = temporary.path().join("good.json");
    let publication = atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS);
    publication
        .write(
            &bad,
            br#"{"nested":[{"secret":"first","secret":"second"}]}"#,
        )
        .unwrap();
    publication
        .write(
            &good,
            br#"{"nested":[null,true,3,"{\"duplicate\":1,\"duplicate\":2}"]}"#,
        )
        .unwrap();
    let before = fs::read(&bad).unwrap();
    let mut output = Vec::new();
    {
        let mut export = EvidenceExport {
            source: temporary.path(),
            output: &mut output,
            counts: (0, 0),
        };
        export.emit(&bad, None).unwrap();
        export.emit(&good, None).unwrap();
        assert_eq!(export.counts, (1, 1));
    }
    let lines = String::from_utf8(output).unwrap();
    assert_eq!(lines.lines().count(), 2);
    assert!(!lines.contains("second"));
    assert!(lines.contains("duplicate"));
    assert_eq!(fs::read(&bad).unwrap(), before);
}

#[test]
fn export_preserves_error_commit_evidence_at_every_durable_boundary() {
    for step in [
        Step::Written,
        Step::BeforeFileSync,
        Step::AfterFileSync,
        Step::BeforePublish,
        Step::AfterPublish,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ] {
        let root = tempfile::tempdir().unwrap();
        let (fresh, _db) = fixture(root.path());
        let output = root.path().join("export.ndjson");
        let error = fresh.export_retired_with(&output, &Fail(step)).unwrap_err();
        let failure = error
            .source()
            .unwrap()
            .downcast_ref::<atomic_fs::Error>()
            .unwrap();
        let published = matches!(
            step,
            Step::AfterPublish | Step::BeforeDirectorySync | Step::AfterDirectorySync
        );
        assert_eq!(failure.published(), published);
        assert_eq!(failure.io_error().raw_os_error(), Some(libc::EIO));
        assert_eq!(output.exists(), published);
        if published {
            let lines: Vec<Value> = fs::read_to_string(&output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(lines.len(), 5);
            assert_eq!(lines[0]["type"], "manifest");
            let before = fs::read(&output).unwrap();
            assert!(fresh.export_retired(&output).is_err());
            assert_eq!(fs::read(&output).unwrap(), before);
        }
        assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".hive-tmp-")
        }));
    }
}

#[test]
fn symlink_parent_export_is_supported_but_cannot_point_inside_retired_root() {
    let root = tempfile::tempdir().unwrap();
    let (fresh, _db) = fixture(root.path());
    let alias = root.path().join("alias");
    symlink(root.path(), &alias).unwrap();
    let output = alias.join("export.ndjson");
    let report = fresh.export_retired(&output).unwrap();
    assert_eq!(report["output"], output.to_str().unwrap());
    assert!(root.path().join("export.ndjson").is_file());
    let forbidden = root.path().join("forbidden");
    symlink(root.path().join("old"), &forbidden).unwrap();
    assert!(fresh.export_retired(&forbidden.join("export")).is_err());
    assert!(!root.path().join("old/export").exists());
}

#[test]
fn cleanup_preserves_inflight_exports_and_completed_generic_temp_names() {
    use crate::state::{self, Lock};
    struct Sweep<'a>(&'a State, &'a Lock, &'a Path);
    impl Barriers for Sweep<'_> {
        fn observe(&self, step: Step) -> io::Result<()> {
            if step == Step::Written {
                let stages: Vec<_> = fs::read_dir(self.2)?
                    .map(|e| e.unwrap().path())
                    .filter(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".hive-tmp-")
                    })
                    .collect();
                assert_eq!(stages.len(), 1);
                self.0.cleanup_preparations(self.1).unwrap();
                assert!(stages[0].exists());
            }
            Ok(())
        }
    }
    for relative in ["", "runtime", "runtime/output"] {
        let root = tempfile::tempdir().unwrap();
        let (fresh, _db) = fixture(root.path());
        state::directory(&fresh.root.join("runtime")).unwrap();
        state::directory(&fresh.root.join("runtime/output")).unwrap();
        let lock = Lock::acquire(&fresh.root.join("runtime/daemon.lock")).unwrap();
        let directory = fresh.root.join(relative);
        let output = directory.join(".hive-tmp-ABC123");
        fresh
            .export_retired_with(&output, &Sweep(&fresh, &lock, &directory))
            .unwrap();
        let bytes = fs::read(&output).unwrap();
        assert!(!bytes.is_empty());
        fresh.cleanup_preparations(&lock).unwrap();
        assert_eq!(fs::read(&output).unwrap(), bytes);
    }
}

#[test]
fn reserved_export_basename_is_rejected_before_source_snapshot_or_reservation() {
    let temp = tempfile::tempdir().unwrap();
    let fresh = State::new(temp.path().join("missing")).unwrap();
    for name in [
        ".hive-tmp-state-ABC123",
        ".hive-tmp-state-0123456789abcdef",
        ".comb-tmp-admin-ABC123",
        ".comb-tmp-admin-0123456789abcdef",
    ] {
        let target = temp.path().join(name);
        assert_eq!(
            fresh.export_retired(&target).unwrap_err().code(),
            "invalid_path"
        );
    }
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn probe_sweep_preserves_active_and_completed_generic_export_names() {
    use crate::state;
    struct Sweep<'a>(&'a State, &'a Path);
    impl Barriers for Sweep<'_> {
        fn observe(&self, step: Step) -> io::Result<()> {
            if step == Step::Written {
                let marker = self.1.join(".hive-tmp-state-ABC123");
                state::private_file(&marker, true).unwrap();
                let stages: Vec<_> = fs::read_dir(self.1)?
                    .map(|e| e.unwrap().path())
                    .filter(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".hive-tmp-")
                            && *p != marker
                    })
                    .collect();
                assert_eq!(stages.len(), 1);
                self.0.sweep_metadata_probes();
                assert!(!marker.exists(), "sweep must run during the active export");
                assert!(stages[0].exists());
            }
            Ok(())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let (fresh, _db) = fixture(root.path());
    let directory = fresh.root.join("native-probes");
    state::directory(&directory).unwrap();
    let output = directory.join(".hive-tmp-ABC123");
    fresh
        .export_retired_with(&output, &Sweep(&fresh, &directory))
        .unwrap();
    let bytes = fs::read(&output).unwrap();
    assert!(!bytes.is_empty());
    fresh.sweep_metadata_probes();
    assert_eq!(fs::read(&output).unwrap(), bytes);
}
