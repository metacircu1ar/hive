//! Exercise real installation preflight cancellation in its own process.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[test]
#[ignore = "Isolated installation fixture driven by its parent cancellation test"]
fn interrupted_installation_child() {
    let original = child_action();
    let mut policy = original;
    policy.sa_sigaction = if std::env::var("HIVE_PROBE_REAP_POLICY").unwrap() == "ignored" {
        libc::SIG_IGN
    } else {
        libc::SIG_DFL
    };
    policy.sa_flags = if policy.sa_sigaction == libc::SIG_IGN {
        0
    } else {
        libc::SA_NOCLDWAIT
    };
    // SAFETY: initialized local action; use a nonempty mask to check restoration.
    unsafe {
        assert_eq!(libc::sigemptyset(&raw mut policy.sa_mask), 0);
        assert_eq!(libc::sigaddset(&raw mut policy.sa_mask, libc::SIGUSR1), 0);
        assert_eq!(
            libc::sigaction(libc::SIGCHLD, &raw const policy, std::ptr::null_mut()),
            0
        );
    }
    let expected = child_action();
    assert_eq!(
        super::run(
            Command::new("/bin/sh").args(["-c", "printf ready"]),
            super::TIMEOUT
        )
        .unwrap(),
        "ready"
    );
    assert_action(&expected);
    let root = PathBuf::from(std::env::var_os("HIVE_PROBE_TMP").unwrap());
    let mut part = crate::tests::fixture(&root, "hive");
    let binary = root.join("hive/bin/hivectl");
    fs::write(
        &binary,
        "#!/bin/sh\ntrap '' INT TERM HUP\nprintf '%s\\n' \"$$\" > \"$HIVE_PROBE_PID\"\nsleep 20\nexit 99\n",
    ).unwrap();
    let record = part.files.get_mut("bin/hivectl").unwrap();
    record.bytes = fs::metadata(&binary).unwrap().len();
    record.sha256 = crate::manifest::hash(&binary).unwrap();
    let prefix = root.join("prefix");
    let error = crate::publish::install(&prefix, &root, &[part], |_| Ok(()))
        .err()
        .unwrap();
    let mut cause = Some(error.as_ref() as &(dyn std::error::Error + 'static));
    let mut interrupted = false;
    while let Some(error) = cause {
        if let Some(native) = error.downcast_ref::<std::io::Error>() {
            interrupted |= native.kind() == std::io::ErrorKind::Interrupted;
        }
        cause = error.source();
    }
    assert!(interrupted, "{error}");
    assert_action(&expected);
    assert!(!prefix.exists());
    // SAFETY: restore the exact action saved before this isolated fixture.
    let restored =
        unsafe { libc::sigaction(libc::SIGCHLD, &raw const original, std::ptr::null_mut()) };
    assert_eq!(restored, 0);
    fs::write(root.join("cancelled"), b"probe reaped before installation").unwrap();
}

#[test]
fn interrupt_reaps_the_probe_group_before_return_without_installation_mutation() {
    for policy in ["ignored", "no_wait"] {
        check_interruption(policy);
    }
}

fn check_interruption(policy: &str) {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("native.pid");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "manifest::probes::cancellation_tests::interrupted_installation_child",
            "--nocapture",
        ])
        .env("HIVE_PROBE_TMP", temp.path())
        .env("HIVE_PROBE_REAP_POLICY", policy)
        .env("HIVE_PROBE_PID", &pid_file);
    let start = Instant::now();
    let error = hive_setup_support::runner::attached_command(
        &mut command,
        None,
        Duration::from_secs(30),
        &|| pid_file.exists(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(
        fs::read(temp.path().join("cancelled")).unwrap(),
        b"probe reaped before installation"
    );
    assert!(!temp.path().join("prefix").exists());
    let pid: libc::pid_t = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    loop {
        // SAFETY: signal zero observes the synthetic group only, without signalling
        // an identity whose owner has already reaped it.
        if unsafe { libc::kill(-pid, 0) } == -1 {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn child_action() -> libc::sigaction {
    // SAFETY: initialized output storage, and null input observes without mutation.
    let mut action = unsafe { std::mem::zeroed() };
    // SAFETY: initialized output storage remains live for the query.
    let queried = unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &raw mut action) };
    assert_eq!(queried, 0);
    action
}

fn assert_action(expected: &libc::sigaction) {
    let actual = child_action();
    assert_eq!(actual.sa_sigaction, expected.sa_sigaction);
    assert_eq!(actual.sa_flags, expected.sa_flags);
    for signal in [
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGCHLD,
    ] {
        // SAFETY: both masks are initialized snapshots from sigaction.
        unsafe {
            assert_eq!(
                libc::sigismember(&raw const actual.sa_mask, signal),
                libc::sigismember(&raw const expected.sa_mask, signal)
            );
        }
    }
}
