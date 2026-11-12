use super::*;
use std::{
    cell::{Cell, RefCell},
    io,
    os::unix::fs::{PermissionsExt, symlink},
};
mod direct;
mod outbox;

const SESSION: &str = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
const PEER: &str = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
type Hook = Box<dyn Fn(Step) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
thread_local! { static DIRECTORY_VISITS: Cell<usize> = const { Cell::new(0) }; }
pub(super) fn directory_visit() {
    DIRECTORY_VISITS.set(DIRECTORY_VISITS.get() + 1);
}
pub(super) fn step(value: Step) -> io::Result<()> {
    HOOK.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(value)))
}
struct Restore(Option<Hook>);
impl Drop for Restore {
    fn drop(&mut self) {
        HOOK.replace(self.0.take());
    }
}
fn with_hook<T>(hook: impl Fn(Step) -> io::Result<()> + 'static, run: impl FnOnce() -> T) -> T {
    let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
    run()
}
fn directory(root: &Path, relative: &Path) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        state::directory(&path).unwrap();
    }
    path
}
fn home(root: &Path, generation: uuid::Uuid) -> PathBuf {
    directory(
        root,
        &Path::new("profiles")
            .join(generation.to_string())
            .join("provider"),
    )
}
fn write(home: &Path, relative: &str) -> PathBuf {
    let relative = Path::new(relative);
    directory(home, relative.parent().unwrap());
    let path = home.join(relative);
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&path, b"native fixture\n")
        .unwrap();
    path
}
fn rollout(id: &str) -> String {
    format!("sessions/2026/11/05/rollout-2026-11-05-{id}.jsonl")
}

fn plan_at(home: &Path, provider: Provider, session: &str, paths: &[(&str, bool)]) -> Result<Plan> {
    let locations: Vec<_> = paths
        .iter()
        .map(|(path, verified)| super::super::locations::Location {
            path: (*path).into(),
            verified: *verified,
        })
        .collect();
    discover(home, provider, session, &locations, &mut Budget::new(None))?
        .ok_or_else(super::super::planning::invalid)
}
fn plan(home: &Path, provider: Provider, session: &str) -> Result<Plan> {
    let path = if provider == Provider::Codex {
        rollout(session)
    } else {
        format!("projects/project/{session}.jsonl")
    };
    plan_at(home, provider, session, &[(&path, true)])
}

#[test]
fn claude_native_backup_modes_do_not_prevent_session_cleanup() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let transcript = write(&home, &format!("projects/project/{SESSION}.jsonl"));
    let backups = [0o644, 0o755].map(|mode| {
        let path = write(&home, &format!("file-history/{SESSION}/backup-{mode}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    });
    let plan = plan(&home, Provider::Claude, SESSION).unwrap();
    assert_eq!(plan.warning, None);
    assert_eq!(plan.files.len(), 3);
    for file in &plan.files {
        assert!(matches!(
            unlink(&home, Provider::Claude, SESSION, file).unwrap(),
            Removed::Deleted(15)
        ));
    }
    assert!(!transcript.exists());
    assert!(backups.iter().all(|path| !path.exists()));
}

#[test]
fn codex_exact_owned_rollout_leaves_peers_globals_and_directories() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let owned = write(&home, &rollout(SESSION));
    let kept = [
        write(&home, &rollout(PEER)),
        write(&home, "history.jsonl"),
        write(&home, "auth.json"),
    ];
    let plan = plan(&home, Provider::Codex, SESSION).unwrap();
    assert_eq!(plan.warning, None);
    assert_eq!(plan.files.len(), 1);
    assert!(matches!(
        unlink(&home, Provider::Codex, SESSION, &plan.files[0]).unwrap(),
        Removed::Deleted(15)
    ));
    assert!(!owned.exists());
    assert!(owned.parent().unwrap().is_dir());
    for path in kept {
        assert_eq!(fs::read(path).unwrap(), b"native fixture\n");
    }
    assert!(matches!(
        unlink(&home, Provider::Codex, SESSION, &plan.files[0]).unwrap(),
        Removed::AlreadyAbsent
    ));
}

#[test]
fn ambiguous_invalid_and_symlinked_sessions_are_kept() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let owned = write(&home, &rollout(SESSION));
    let archived = write(
        &home,
        &format!("archived_sessions/rollout-2026-11-05-{SESSION}.jsonl"),
    );
    let ambiguous = plan(&home, Provider::Codex, SESSION).unwrap();
    assert_eq!(ambiguous.warning, Some(Warning::AmbiguousSession));
    assert!(ambiguous.files.is_empty());
    fs::remove_file(archived).unwrap();
    for id in [
        "../escape".to_owned(),
        SESSION.to_uppercase(),
        SESSION.replace('-', ""),
    ] {
        assert_eq!(
            plan(&home, Provider::Codex, &id).unwrap().warning,
            Some(Warning::InvalidSessionIdentity)
        );
    }
    let peer = write(&home, &rollout(PEER));
    fs::remove_file(&owned).unwrap();
    symlink(&peer, &owned).unwrap();
    let unsafe_plan = plan(&home, Provider::Codex, SESSION).unwrap();
    assert_eq!(unsafe_plan.warning, Some(Warning::UnsafePath));
    assert!(unsafe_plan.files.is_empty());
    assert!(peer.exists());
}

#[test]
fn replaced_file_or_ancestor_and_outbox_escape_never_unlink() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let owned = write(&home, &rollout(SESSION));
    let original = plan(&home, Provider::Codex, SESSION)
        .unwrap()
        .files
        .remove(0);
    write(&home, &rollout(SESSION));
    assert!(matches!(
        unlink(&home, Provider::Codex, SESSION, &original).unwrap(),
        Removed::Retained(Warning::IdentityChanged)
    ));
    let current = plan(&home, Provider::Codex, SESSION)
        .unwrap()
        .files
        .remove(0);
    fs::rename(home.join("sessions"), home.join("saved-sessions")).unwrap();
    symlink(home.join("saved-sessions"), home.join("sessions")).unwrap();
    assert!(matches!(
        unlink(&home, Provider::Codex, SESSION, &current).unwrap(),
        Removed::Retained(Warning::UnsafePath)
    ));
    assert!(owned.exists());
    let malicious = File {
        path: PathBuf::from("../auth.json"),
        ..current
    };
    assert_eq!(
        unlink(&home, Provider::Codex, SESSION, &malicious)
            .err()
            .unwrap()
            .code(),
        "database_error"
    );
}

#[test]
fn claude_owns_exact_session_artifacts_but_no_shared_indexes_or_peers() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let names = [
        format!("projects/project/{SESSION}.jsonl"),
        format!("projects/project/{SESSION}/subagents/agent-worker.jsonl"),
        format!("projects/project/{SESSION}/tool-results/result.txt"),
        format!("file-history/{SESSION}/snapshot@v1"),
        format!("session-env/{SESSION}/snapshot"),
        format!("tasks/{SESSION}/1.json"),
        format!("debug/{SESSION}.txt"),
        format!("todos/{SESSION}-agent-{SESSION}.json"),
        format!("todos/{SESSION}-agent-worker.json"),
    ];
    let owned: Vec<_> = names.iter().map(|name| write(&home, name)).collect();
    let kept = [
        write(&home, &format!("projects/project/{PEER}.jsonl")),
        write(&home, "projects/project/sessions-index.json"),
        write(&home, "history.jsonl"),
        write(&home, "credentials.json"),
        write(&home, &format!("todos/{SESSION}-agent-unproved.json")),
    ];
    let planned = plan(&home, Provider::Claude, SESSION).unwrap();
    assert_eq!(planned.warning, None);
    assert_eq!(planned.files.len(), names.len());
    for file in &planned.files {
        assert!(matches!(
            unlink(&home, Provider::Claude, SESSION, file).unwrap(),
            Removed::Deleted(_)
        ));
    }
    for path in owned {
        assert!(!path.exists());
        assert!(path.parent().unwrap().is_dir());
    }
    for path in kept {
        assert_eq!(fs::read(path).unwrap(), b"native fixture\n");
    }
    write(&home, &format!("projects/project/{SESSION}.jsonl"));
    write(&home, &format!("projects/other/{SESSION}.jsonl"));
    let ambiguous = plan_at(
        &home,
        Provider::Claude,
        SESSION,
        &[
            (&format!("projects/project/{SESSION}.jsonl"), true),
            (&format!("projects/other/{SESSION}.jsonl"), false),
        ],
    )
    .unwrap();
    assert_eq!(ambiguous.warning, Some(Warning::AmbiguousSession));
    assert!(ambiguous.files.is_empty());
}

#[test]
fn regular_file_refusal_keeps_its_exact_diagnostic() {
    let root = crate::runtime::test_directory();
    let error = metadata(root.path(), false).unwrap_err();
    assert_eq!(
        error.message(),
        "Native session cleanup requires owned regular files without symlinks or extra hard links."
    );
}
mod encoding;
