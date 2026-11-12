use super::*;
use hive_setup_support::runner::{Runner as _, SystemRunner};
use std::error::Error as _;
use std::{fs, io, path::PathBuf, time::Instant};

#[test]
#[ignore = "Isolated native manager/maintenance signal-policy fixture"]
fn setup_owner_child() {
    let original = disposition();
    let mut policy = original;
    let ignored = std::env::var("HIVE_SETUP_COMB_POLICY").unwrap() == "ignored";
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
    let args = ["-c".into(), "printf '{\"ready\":true}'".into()];
    let output: serde_json::Value =
        call(Path::new("/bin/sh"), &args, Duration::from_secs(2), &[]).unwrap();
    assert_eq!(output, serde_json::json!({"ready": true}));
    assert_restored(&expected);
    let output: serde_json::Value = progress(Path::new("/bin/sh"), &args, &[]).unwrap();
    assert_eq!(output, serde_json::json!({"ready": true}));
    assert_restored(&expected);
    let root = PathBuf::from(std::env::var_os("HIVE_SETUP_COMB_ROOT").unwrap());
    let args = [
        "--ignored".into(),
        "--exact".into(),
        "cli::setup::parts::tests::comb_owner_child".into(),
        "--nocapture".into(),
    ];
    let binary = std::env::current_exe().unwrap();
    let mode = std::env::var("HIVE_SETUP_COMB_MODE").unwrap();
    let result = if mode == "progress" {
        progress::<serde_json::Value>(&binary, &args, &[])
    } else {
        call::<serde_json::Value>(
            &binary,
            &args,
            if mode == "timeout" {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(20)
            },
            &[],
        )
    };
    assert_eq!(
        result
            .unwrap_err()
            .source()
            .unwrap()
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
fn comb_owner_child() {
    let root = PathBuf::from(std::env::var_os("HIVE_SETUP_COMB_ROOT").unwrap());
    fs::write(root.join("owner-pid"), std::process::id().to_string()).unwrap();
    let args = [
        "-c".into(),
        "trap '' INT TERM HUP; printf %s $$ > \"$HIVE_SETUP_COMB_PID\"; sleep 30".into(),
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
fn setup_comb_cancel_and_timeout_reap_nested_manager_and_restore_autoreap() {
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
            "cli::setup::parts::tests::setup_owner_child",
            "--nocapture",
        ])
        .env("HIVE_SETUP_COMB_ROOT", root.path())
        .env("HIVE_SETUP_COMB_PID", &pid_file)
        .env("HIVE_SETUP_COMB_POLICY", policy)
        .env("HIVE_SETUP_COMB_MODE", mode);
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
    assert!(began.elapsed() < Duration::from_secs(5));
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
