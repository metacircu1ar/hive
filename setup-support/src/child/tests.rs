use super::*;
use std::{os::unix::process::CommandExt, process::Command};

struct Fixture(Child);
impl Fixture {
    fn new(body: &str) -> Self {
        Self(
            Command::new("/bin/sh")
                .args(["-c", body])
                .process_group(0)
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Never signal a released PID. All expected test paths reap explicitly.
        if has_exited(&self.0).is_ok() {
            let _ = kill_owned(&mut self.0, |child| {
                process::kill_process_group(Pid::from_child(child), Signal::KILL)
                    .map_err(Into::into)
            });
            let _ = reap_until(&mut self.0, Instant::now() + FORCED_REAP_GRACE);
        }
    }
}

#[test]
fn expired_reap_budget_does_not_wait_for_a_live_child() {
    let mut child = Fixture::new("exec sleep 30");
    let started = Instant::now();
    assert!(reap_until(&mut child.0, started).unwrap().is_none());
    assert!(started.elapsed() < Duration::from_millis(200));
    assert!(!has_exited(&child.0).unwrap());
}

#[test]
fn a_pending_stop_is_not_exit_evidence_and_cannot_consume_the_owned_pid() {
    let child = Fixture::new("kill -STOP $$; exec sleep 30");
    let deadline = Instant::now() + FORCED_REAP_GRACE;
    while !has_stopped(&child.0).unwrap() && Instant::now() < deadline {
        thread::sleep(REAP_POLL);
    }
    assert!(has_stopped(&child.0).unwrap());
    assert!(!has_exited(&child.0).unwrap());
    assert!(has_stopped(&child.0).unwrap());
}

#[test]
fn an_exited_child_is_not_stopped_and_its_status_remains_owned() {
    let mut child = Fixture::new("exit 73");
    let deadline = Instant::now() + FORCED_REAP_GRACE;
    while !has_exited(&child.0).unwrap() && Instant::now() < deadline {
        thread::sleep(REAP_POLL);
    }
    assert!(has_exited(&child.0).unwrap());
    assert!(!has_stopped(&child.0).unwrap());
    assert!(!has_stopped(&child.0).unwrap());
    assert!(has_exited(&child.0).unwrap());
    assert_eq!(child.0.wait().unwrap().code(), Some(73));
}

#[test]
fn failed_signal_does_not_discard_a_waitable_status() {
    let mut child = Fixture::new("exit 73");
    let deadline = Instant::now() + FORCED_REAP_GRACE;
    while !has_exited(&child.0).unwrap() && Instant::now() < deadline {
        thread::sleep(REAP_POLL);
    }
    assert!(has_exited(&child.0).unwrap());
    let attempt = kill_owned(&mut child.0, |_| {
        Err(io::Error::from_raw_os_error(libc::EPERM))
    });
    assert_eq!(
        attempt.group_error.unwrap().raw_os_error(),
        Some(libc::EPERM)
    );
    assert!(attempt.leader_error.is_none());
    assert_eq!(
        reap_until(&mut child.0, deadline).unwrap().unwrap().code(),
        Some(73)
    );
}

#[test]
fn ownership_loss_forbids_both_signal_paths() {
    let mut child = Fixture::new("exit 0");
    child.0.wait().unwrap();
    assert_eq!(
        has_stopped(&child.0).unwrap_err().raw_os_error(),
        Some(libc::ECHILD)
    );
    let attempt = kill_owned(&mut child.0, |_| panic!("released child was signalled"));
    assert_eq!(
        attempt.ownership_error.unwrap().raw_os_error(),
        Some(libc::ECHILD)
    );
    assert!(attempt.leader_error.is_none());
    assert_eq!(
        reap_until(&mut child.0, Instant::now())
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn failed_group_signal_still_kills_and_reaps_the_owned_leader() {
    let mut child = Fixture::new("exec sleep 30");
    let attempt = kill_owned(&mut child.0, |_| {
        Err(io::Error::from_raw_os_error(libc::ESRCH))
    });
    assert!(attempt.ownership_error.is_none());
    assert!(attempt.leader_error.is_none());
    assert!(
        reap_until(&mut child.0, Instant::now() + FORCED_REAP_GRACE)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_moved_leader_is_killed_with_or_without_an_old_group_member() {
    use std::{io::Write, process::Stdio};
    for populated in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let sibling = Fixture::new("exec sleep 30");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "child::tests::moved_leader_fixture", "--ignored"])
            .env("HIVE_MOVED_GROUP", sibling.0.id().to_string())
            .env("HIVE_MOVED_READY", &ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .process_group(0);
        let mut leader = Fixture(command.spawn().unwrap());
        let mut old_member = populated.then(|| {
            Fixture(
                Command::new("/bin/sleep")
                    .arg("30")
                    .process_group(i32::try_from(leader.0.id()).unwrap())
                    .spawn()
                    .unwrap(),
            )
        });
        leader.0.stdin.take().unwrap().write_all(b"G").unwrap();
        let deadline = Instant::now() + FORCED_REAP_GRACE;
        while !ready.exists() && Instant::now() < deadline {
            thread::sleep(REAP_POLL);
        }
        assert!(ready.exists());
        let attempt = kill_owned(&mut leader.0, |child| {
            process::kill_process_group(Pid::from_child(child), Signal::KILL).map_err(Into::into)
        });
        assert!(attempt.ownership_error.is_none());
        assert!(attempt.leader_error.is_none());
        if populated {
            assert!(attempt.group_error.is_none());
        } else {
            assert_eq!(
                attempt.group_error.unwrap().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        assert!(
            reap_until(&mut leader.0, Instant::now() + FORCED_REAP_GRACE)
                .unwrap()
                .is_some()
        );
        if let Some(member) = &mut old_member {
            assert!(
                reap_until(&mut member.0, Instant::now() + FORCED_REAP_GRACE)
                    .unwrap()
                    .is_some()
            );
        }
        assert!(
            !has_exited(&sibling.0).unwrap(),
            "unrelated sibling group was signalled"
        );
    }
}

#[test]
#[ignore = "native child fixture for the isolated moved-group regression"]
fn moved_leader_fixture() {
    use std::io::Read;
    let mut grant = [0];
    std::io::stdin().read_exact(&mut grant).unwrap();
    let group: i32 = std::env::var("HIVE_MOVED_GROUP").unwrap().parse().unwrap();
    process::setpgid(None, Pid::from_raw(group)).unwrap();
    std::fs::write(std::env::var_os("HIVE_MOVED_READY").unwrap(), b"ready").unwrap();
    thread::sleep(Duration::from_secs(30));
}
