//! Independent kernel identity for the Bash workload, including PID namespaces.
use super::*;
use hive::state::ProcessIdentity;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Observation {
    process: ProcessIdentity,
    guard_inputs_removed: bool,
    #[cfg(target_os = "linux")]
    namespace: u64,
}

#[test]
#[ignore = "fixture child executed only by the native interrupt canary"]
fn native_shell_identity() {
    if std::env::var_os("HIVE_NATIVE_IDENTITY_CHILD").as_deref() != Some("1".as_ref()) {
        return;
    }
    let observation = Observation {
        process: ProcessIdentity::current().unwrap(),
        guard_inputs_removed: std::env::var_os("HIVE_SHELL_GUARD_SOCKET").is_none()
            && std::env::var_os("HIVE_SHELL_GUARD_FD").is_none(),
        #[cfg(target_os = "linux")]
        namespace: {
            use std::os::unix::fs::MetadataExt;
            fs::metadata("/proc/self/ns/pid").unwrap().ino()
        },
    };
    fs::write(
        "native-interrupt-identity",
        serde_json::to_vec(&observation).unwrap(),
    )
    .unwrap();
    fs::write("native-interrupt-ready", "ready").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !Path::new("native-interrupt-release").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "native fixture child deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn running(workspace: &Path) -> ProcessIdentity {
    let observation: Observation =
        serde_json::from_slice(&fs::read(workspace.join("native-interrupt-identity")).unwrap())
            .unwrap();
    assert!(
        observation.guard_inputs_removed,
        "Bash inherited private guard inputs"
    );
    #[cfg(target_os = "macos")]
    let process = {
        assert_eq!(
            ProcessIdentity::read(observation.process.pid).unwrap(),
            Some(observation.process.clone())
        );
        observation.process
    };
    #[cfg(target_os = "linux")]
    let process = linux_process(observation);
    assert!(!process.absent().unwrap());
    #[cfg(target_os = "linux")]
    assert_supervised(process.pid);
    process
}

#[cfg(target_os = "linux")]
fn assert_supervised(pid: u32) {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let parent: u32 = status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The actual supervisor creates this child only after registration and
    // releases its execution gate only after the daemon attaches the identity.
    let image = fs::read_link(format!("/proc/{parent}/exe")).unwrap();
    assert!(
        image.ends_with("control/hive-shell"),
        "Bash lacks its Hive supervisor"
    );
}

#[cfg(target_os = "linux")]
fn linux_process(observation: Observation) -> ProcessIdentity {
    use std::os::unix::fs::MetadataExt;
    let mut matches = Vec::new();
    for entry in fs::read_dir("/proc").unwrap() {
        let entry = entry.unwrap();
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let namespace = match fs::metadata(entry.path().join("ns/pid")) {
            Ok(metadata) => metadata.ino(),
            Err(error) if transient(&error) => continue,
            Err(error) => panic!("read process namespace: {error}"),
        };
        if namespace != observation.namespace {
            continue;
        }
        let status = match fs::read_to_string(entry.path().join("status")) {
            Ok(status) => status,
            Err(error) if transient(&error) => continue,
            Err(error) => panic!("read process status: {error}"),
        };
        let local = status
            .lines()
            .find_map(|line| line.strip_prefix("NSpid:"))
            .and_then(|line| line.split_whitespace().last())
            .and_then(|pid| pid.parse::<u32>().ok());
        if local != Some(observation.process.pid) {
            continue;
        }
        if let Some(identity) = ProcessIdentity::read(pid).unwrap()
            && identity.birth == observation.process.birth
        {
            matches.push(identity);
        }
    }
    assert_eq!(matches.len(), 1, "exactly one running namespaced child");
    matches.pop().unwrap()
}

#[cfg(target_os = "linux")]
fn transient(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    ) || error.raw_os_error() == Some(libc::ESRCH)
}
