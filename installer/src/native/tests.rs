use super::*;
use hive_setup_support::runner::{Runner as _, SystemRunner};
use std::{fs, io, path::PathBuf, time::Instant};

#[test]
#[ignore = "Isolated native manager/maintenance signal-policy fixture"]
fn native_owner_child() {
    let original = disposition();
    let mut policy = original;
    let ignored = std::env::var("HIVE_MANAGER_POLICY").unwrap() == "ignored";
    policy.sa_sigaction = if ignored {
        libc::SIG_IGN
    } else {
        libc::SIG_DFL
    };
    policy.sa_flags = if ignored { 0 } else { libc::SA_NOCLDWAIT };
    // SAFETY: initialized process-local signal snapshot in an isolated test process.
    unsafe {
        assert_eq!(libc::sigaddset(&raw mut policy.sa_mask, libc::SIGUSR1), 0);
        assert_eq!(
            libc::sigaction(libc::SIGCHLD, &raw const policy, std::ptr::null_mut()),
            0
        );
    }
    let expected = disposition();
    let args = ["-c".into(), "printf ready; exit 17".into()];
    let output = SystemRunner(Command::spawn).run("/bin/sh", &args).unwrap();
    assert_eq!((output.code, output.stdout.as_str()), (17, "ready"));
    assert_restored(&expected);
    let root = PathBuf::from(std::env::var_os("HIVE_MANAGER_ROOT").unwrap());
    let args = [
        "--ignored".into(),
        "--exact".into(),
        "native::tests::nested_manager_owner_child".into(),
        "--nocapture".into(),
    ];
    let binary = std::env::current_exe().unwrap();
    let binary = binary.to_str().unwrap();
    let mode = std::env::var("HIVE_MANAGER_MODE").unwrap();
    let result = if mode == "progress" {
        progress(binary, &args)
    } else {
        capture(
            binary,
            &args,
            if mode == "timeout" {
                Duration::from_millis(250)
            } else {
                Duration::from_secs(20)
            },
        )
    };
    assert_eq!(
        result
            .unwrap_err()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        if mode == "timeout" {
            io::ErrorKind::TimedOut
        } else {
            io::ErrorKind::Interrupted
        }
    );
    assert_restored(&expected);
    // SAFETY: restore the exact original disposition before the isolated process exits.
    let restored =
        unsafe { libc::sigaction(libc::SIGCHLD, &raw const original, std::ptr::null_mut()) };
    assert_eq!(restored, 0);
    fs::write(root.join("done"), b"owned child reaped").unwrap();
}

#[test]
#[ignore = "Service-like inner owner of a separately grouped ignoring manager"]
fn nested_manager_owner_child() {
    let root = PathBuf::from(std::env::var_os("HIVE_MANAGER_ROOT").unwrap());
    fs::write(root.join("owner-pid"), std::process::id().to_string()).unwrap();
    let args = [
        "-c".into(),
        "trap '' INT TERM HUP; printf %s $$ > \"$HIVE_MANAGER_PID\"; sleep 30".into(),
    ];
    let error = SystemRunner(Command::spawn)
        .run("/bin/sh", &args)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    fs::write(
        root.join("inner-retired"),
        b"manager reaped before owner exit",
    )
    .unwrap();
    std::process::exit(76);
}

#[test]
fn native_manager_and_maintenance_cancel_and_restore_inherited_autoreap() {
    for policy in ["ignored", "no_wait"] {
        for mode in ["capture", "progress", "timeout"] {
            check_owner(policy, mode);
        }
    }
}

fn check_owner(policy: &str, mode: &str) {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("pid");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "native::tests::native_owner_child",
            "--nocapture",
        ])
        .env("HIVE_MANAGER_ROOT", root.path())
        .env("HIVE_MANAGER_PID", &pid_file)
        .env("HIVE_MANAGER_POLICY", policy)
        .env("HIVE_MANAGER_MODE", mode);
    let began = Instant::now();
    let result = hive_setup_support::runner::attached_command(
        &mut command,
        None,
        Duration::from_secs(20),
        &|| mode != "timeout" && pid_file.exists(),
    );
    if mode == "timeout" {
        assert_eq!(result.unwrap().code, 0);
    } else {
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }
    assert_eq!(
        fs::read(root.path().join("done")).unwrap(),
        b"owned child reaped"
    );
    assert_eq!(
        fs::read(root.path().join("inner-retired")).unwrap(),
        b"manager reaped before owner exit"
    );
    for file in [pid_file, root.path().join("owner-pid")] {
        let group: libc::pid_t = fs::read_to_string(file).unwrap().parse().unwrap();
        assert_group_gone(group, began);
    }
}

fn assert_group_gone(group: libc::pid_t, began: Instant) {
    loop {
        // SAFETY: signal zero observes only the synthetic group; no post-reap signal.
        if unsafe { libc::kill(-group, 0) } == -1 {
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn disposition() -> libc::sigaction {
    // SAFETY: zero-initialize output storage, query without changing the disposition.
    let mut action = unsafe { std::mem::zeroed() };
    // SAFETY: output storage is initialized; null input only observes this process.
    let queried = unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &raw mut action) };
    assert_eq!(queried, 0);
    action
}
fn assert_restored(expected: &libc::sigaction) {
    let actual = disposition();
    assert_eq!(actual.sa_sigaction, expected.sa_sigaction);
    assert_eq!(actual.sa_flags, expected.sa_flags);
    // SAFETY: both masks were populated by successful sigaction queries.
    let actual_member = unsafe { libc::sigismember(&raw const actual.sa_mask, libc::SIGUSR1) };
    // SAFETY: the expected mask is another populated sigaction snapshot.
    let expected_member = unsafe { libc::sigismember(&raw const expected.sa_mask, libc::SIGUSR1) };
    assert_eq!(actual_member, expected_member);
}
