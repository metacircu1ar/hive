use super::*;
use std::os::fd::{FromRawFd, OwnedFd};

#[test]
fn native_exec_preserves_only_explicitly_retained_descriptors() {
    let file = tempfile::tempfile().unwrap();
    // Use a fresh high descriptor, never overwrite a descriptor owned by another test.
    let fd = {
        let _fork = os::fork_guard().unwrap();
        // SAFETY: F_DUPFD duplicates the valid file into a new, unowned descriptor.
        let raw = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 192) };
        assert!(raw >= 192);
        // SAFETY: fcntl returned a newly owned descriptor above.
        unsafe { OwnedFd::from_raw_fd(raw) }
    };
    for retained in [false, true] {
        let cache = test_cache().unwrap();
        let mut command =
            crate::test_support::rerun_isolated("launch::tests::fds::descriptor_probe", true);
        command
            .env("HIVE_TEST_FD", fd.as_raw_fd().to_string())
            .env("HIVE_TEST_FD_RETAINED", retained.to_string());
        let allowed: Vec<_> = retained.then_some(fd.as_raw_fd()).into_iter().collect();
        let child = spawn(&cache, &command, true, &allowed, |_| Ok(())).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
#[ignore = "native descriptor fixture reached through the trusted executed shim"]
fn descriptor_probe() {
    let fd: i32 = std::env::var("HIVE_TEST_FD").unwrap().parse().unwrap();
    let retained: bool = std::env::var("HIVE_TEST_FD_RETAINED")
        .unwrap()
        .parse()
        .unwrap();
    // SAFETY: F_GETFD only queries an integer descriptor, including a closed one.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_eq!(flags >= 0, retained);
    if retained {
        assert_eq!(flags & libc::FD_CLOEXEC, 0);
    } else {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EBADF)
        );
    }
}

#[test]
fn rejected_fd_allowlist_reports_a_complete_before_exec_failure() {
    use std::io::{Read, Write};
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("must-not-run");
    let (mut parent, gate) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let fd = gate.as_raw_fd();
    let cache = test_cache().unwrap();
    let mut command = Command::new(cache.image().unwrap());
    command
        .arg(fd.to_string())
        .arg("invalid-descriptor")
        .args(["/bin/sh", "-c", "echo unsafe > \"$1\"", "fd-test"])
        .arg(&marker);
    os::configure_launcher(&mut command, false, vec![fd]);
    let mut child = crate::test_support::ChildGuard(os::spawn(&mut command).unwrap());
    drop(gate);
    parent.write_all(b"G").unwrap();
    let mut status = [0; STATUS_BYTES];
    parent.read_exact(&mut status).unwrap();
    assert_eq!(&status[..TAG_BYTES], STATUS_TAG);
    assert_eq!(
        i32::from_ne_bytes(status[TAG_BYTES..].try_into().unwrap()),
        libc::EINVAL
    );
    assert_eq!(child.wait().unwrap().code(), Some(126));
    assert!(!marker.exists());
}

#[test]
fn invalid_retained_fd_is_rejected_before_native_execution() {
    let cache = test_cache().unwrap();
    let mut authorized = false;
    let result = spawn(&cache, &Command::new("/bin/true"), true, &[-1], |_| {
        authorized = true;
        Ok(())
    });
    assert!(!authorized);
    let error = result.unwrap_err();
    assert_eq!(error.code(), "spawn_failed");
    assert_eq!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(libc::EBADF)
    );
}
