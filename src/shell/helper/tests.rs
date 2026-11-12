use super::*;

#[cfg(target_os = "macos")]
#[test]
fn cleanup_preserves_status_after_killing_background_members() {
    for body in ["exit 73", "sleep 30 & exit 73"] {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", body]);
        os::configure_process_group(&mut command);
        let child = os::spawn(&mut command).unwrap();
        let pid = child.id();
        let mut diagnostics = Diagnostics::default();
        let group = Group {
            child: Some(child),
            diagnostics: &mut diagnostics,
            cleanup_deadline: None,
        };
        if env::var_os("HIVE_TEST_DENIED_LISTING").is_some() {
            crate::test_support::wait_until(Duration::from_secs(3), CLEANUP_POLL_INTERVAL, || {
                os::has_exited(group.child.as_ref().unwrap())
                    .unwrap()
                    .then_some(())
            });
            assert!(os::shell::group_only_leader(pid).is_err());
        }
        let result = wait_cleanup(group, &process::Cancellation::default());
        assert_eq!(result.unwrap(), 73);
        assert!(os::group_absent(pid).unwrap());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_cleanup_works_with_native_process_rules_and_denied_listing() {
    for denied in [false, true] {
        let mut profile = String::from(concat!(
            "(version 1) (allow default) (deny process*) (allow process-fork) ",
            "(allow process-exec) (allow process-info* (target same-sandbox)) ",
            "(allow signal (target same-sandbox))",
        ));
        if denied {
            profile.push_str(" (deny process-info-listpids)");
        }
        let fixture = crate::test_support::rerun_isolated(
            "shell::helper::tests::cleanup_preserves_status_after_killing_background_members",
            false,
        );
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .args(["-p", &profile])
            .arg(fixture.get_program())
            .args(fixture.get_args());
        if denied {
            command.env("HIVE_TEST_DENIED_LISTING", "1");
        }
        crate::test_support::run_isolated(&mut command);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_group_without_its_leader_cannot_prove_only_the_leader_remains() {
    use crate::test_support::ChildGuard;
    let mut leader = ChildGuard(
        os::spawn(
            Command::new("/bin/sh")
                .args(["-c", "read value"])
                .stdin(Stdio::piped())
                .process_group(0),
        )
        .unwrap(),
    );
    let pid = leader.id();
    let _member = ChildGuard(
        os::spawn(
            Command::new("/bin/sleep")
                .arg("30")
                .process_group(i32::try_from(pid).unwrap()),
        )
        .unwrap(),
    );
    leader.stdin.take();
    leader.wait().unwrap();
    assert!(!os::shell::group_only_leader(pid).unwrap());
}
