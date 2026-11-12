use super::*;

#[test]
fn poll_observes_readiness_and_errors_before_an_expired_deadline() {
    let deadline = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    for expected in [Ok(true), Ok(false), Err("observation failed")] {
        let mut observations = 0;
        let actual = wait_until(deadline, Duration::from_secs(60), || {
            observations += 1;
            expected
        });
        assert_eq!(actual, expected);
        assert_eq!(observations, 1);
    }
}

#[test]
fn group_cleanup_retains_each_callers_receipt_policy() {
    for (policy, confirmed) in [
        (CleanupReceipt::GroupAbsence, true),
        (CleanupReceipt::Unconfirmed, false),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::state::State::new(tmp.path().join("state")).unwrap();
        let mut operation = state.metadata_probe().unwrap();
        let launcher = crate::launch::test_cache().unwrap();
        let super::super::Spawned { child, receipt, .. } = super::super::spawn(
            &launcher,
            std::process::Command::new("/bin/sleep").arg("10"),
            &mut operation,
            true,
            None,
        )
        .unwrap();
        let pid = child.id();
        drop(OwnedGroup::new(child, receipt, policy));
        assert!(os::group_absent(pid).unwrap());
        assert_eq!(operation.children_confirmed(), confirmed);
    }
}

#[cfg(target_os = "macos")]
fn owned_command(
    command: &mut std::process::Command,
) -> (tempfile::TempDir, crate::state::Operation, OwnedGroup) {
    let tmp = tempfile::tempdir().unwrap();
    let state = crate::state::State::new(tmp.path().join("state")).unwrap();
    let mut operation = state.metadata_probe().unwrap();
    let launcher = crate::launch::test_cache().unwrap();
    let super::super::Spawned { child, receipt, .. } =
        super::super::spawn(&launcher, command, &mut operation, true, None).unwrap();
    (
        tmp,
        operation,
        OwnedGroup::new(child, receipt, CleanupReceipt::GroupAbsence),
    )
}

#[cfg(target_os = "macos")]
#[test]
fn zombie_only_group_permission_denial_is_expected_but_not_a_cleanup_receipt() {
    let (_tmp, operation, mut owned) =
        owned_command(std::process::Command::new("/bin/sh").args(["-c", "exit 0"]));
    assert!(
        wait_until(
            Instant::now() + Duration::from_secs(3),
            CAPTURE_POLL_INTERVAL,
            || owned.exited()
        )
        .unwrap()
    );
    let native = os::signal_group(&owned.child, Signal::KILL);
    if let Err(error) = native {
        assert!(signal_already_exited(&owned.child, &error).unwrap());
    }
    assert!(
        signal_already_exited(
            &owned.child,
            &std::io::Error::from_raw_os_error(libc::EPERM)
        )
        .unwrap()
    );
    assert!(!owned.reaped);
    let pid = owned.child.id();
    drop(owned);
    assert!(os::group_absent(pid).unwrap());
    assert!(operation.children_confirmed());
}

#[cfg(target_os = "macos")]
#[test]
fn permission_denial_with_a_live_leader_or_descendant_stays_unexpected() {
    let permission = std::io::Error::from_raw_os_error(libc::EPERM);
    let (_tmp, _operation, live) =
        owned_command(std::process::Command::new("/bin/sleep").arg("30"));
    assert!(!signal_already_exited(&live.child, &permission).unwrap());
    drop(live);
    let (_parent_tmp, _parent_operation, mut parent) = owned_command(
        std::process::Command::new("/bin/sh").args(["-c", "sleep 30 >/dev/null 2>&1 & exit 0"]),
    );
    assert!(
        wait_until(
            Instant::now() + Duration::from_secs(3),
            CAPTURE_POLL_INTERVAL,
            || parent.exited()
        )
        .unwrap()
    );
    assert!(!signal_already_exited(&parent.child, &permission).unwrap());
    let pid = parent.child.id();
    drop(parent);
    assert!(os::group_absent(pid).unwrap());
}
