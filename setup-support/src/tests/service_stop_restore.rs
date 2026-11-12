//! Service stop restore regressions.
use super::*;

#[test]
fn failed_stop_never_applies_root_changes() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    ctl.start(&|| Ok(mock.ready.get())).unwrap();
    mock.failure.set(true);
    let changed = Cell::new(false);
    assert!(
        ctl.paused(&|| Ok(mock.ready.get()), || {
            changed.set(true);
            Ok(())
        })
        .is_err()
    );
    assert!(!changed.get());
    assert!(mock.ready.get());
}

#[test]
fn lifecycle_is_idempotent_owned_and_restores_only_running_services() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    let health = || Ok(mock.ready.get());
    ctl.install(&spec).unwrap();
    let bytes = files::read(&spec.unit).unwrap();
    ctl.install(&spec).unwrap();
    assert_eq!(files::read(&spec.unit).unwrap(), bytes);
    ctl.start(&health).unwrap();
    assert!(ctl.status(&health).unwrap().ready);
    let calls = mock
        .calls
        .borrow()
        .iter()
        .filter(|c| matches!(c[0].as_str(), "bootstrap" | "kickstart"))
        .count();
    ctl.start(&health).unwrap();
    assert_eq!(
        calls,
        mock.calls
            .borrow()
            .iter()
            .filter(|c| matches!(c[0].as_str(), "bootstrap" | "kickstart"))
            .count()
    );
    let result: crate::Result<()> = ctl.paused(&health, || {
        assert!(!mock.ready.get());
        Err(crate::invalid("activation failed"))
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("activation failed")
    );
    assert!(mock.ready.get());
    ctl.stop(&health).unwrap();
    ctl.paused(&health, || Ok(())).unwrap();
    assert!(!mock.ready.get());
    ctl.uninstall(&health).unwrap();
    ctl.uninstall(&health).unwrap();
    assert!(!spec.unit.exists());
    assert!(!spec.descriptor.exists());
}

#[test]
fn interrupted_uninstall_only_finishes_after_confirmed_stop() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    files::remove(&spec.unit, &spec.render().unwrap()).unwrap();
    mock.loaded.set(true);
    assert!(ctl.uninstall(&|| Ok(false)).is_err());
    assert!(spec.descriptor.exists());
    mock.loaded.set(false);
    ctl.uninstall(&|| Ok(false)).unwrap();
    assert!(!spec.descriptor.exists());
}

#[test]
fn retained_installer_guard_blocks_other_mutations_and_restores_exact_service() {
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    let health = || Ok(runner.ready.get());
    ctl.install(&spec).unwrap();
    ctl.start(&health).unwrap();
    let held = ctl.hold().unwrap();
    assert!(held.loaded(&runner).unwrap());
    assert!(ctl.stop(&health).is_err());
    held.stop(&runner, &health).unwrap();
    assert!(ctl.start(&health).is_err());
    held.start(&runner, &health).unwrap();
    held.uninstall(&runner, &health).unwrap();
    assert!(!spec.descriptor.exists());
    assert!(ctl.install(&spec).is_err());
    held.install(&runner).unwrap();
    held.start(&runner, &health).unwrap();
    drop(held);
    ctl.uninstall(&health).unwrap();
    let recovery = ctl.hold_expected(&spec).unwrap();
    recovery.install(&runner).unwrap();
    recovery.start(&runner, &health).unwrap();
    assert!(runner.ready.get());
    drop(recovery);
    ctl.uninstall(&health).unwrap();
}

#[test]
fn installer_setup_preserves_stopped_but_starts_new_scopes_and_refuses_uncertainty() {
    use service::StartDecision;
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    ctl.install(&spec).unwrap();
    let health = || Ok(runner.ready.get());
    assert_eq!(
        ctl.start_for_setup(&health, true).unwrap(),
        StartDecision::PreservedStopped
    );
    assert!(!runner.loaded.get());
    // The caller passes false for a just-installed scope.
    assert_eq!(
        ctl.start_for_setup(&health, false).unwrap(),
        StartDecision::Started
    );
    assert_eq!(
        ctl.start_for_setup(&health, true).unwrap(),
        StartDecision::AlreadyLoaded
    );
    ctl.stop(&health).unwrap();
    runner.unavailable.set(true);
    assert!(ctl.start_for_setup(&health, true).is_err());
    runner.unavailable.set(false);
    assert!(ctl.start_for_setup(&|| Ok(true), true).is_err());
    files::write(
        &spec.descriptor.with_extension("failure.json"),
        b"{\"code\":\"config_invalid\"}",
        false,
    )
    .unwrap();
    assert!(ctl.start_for_setup(&health, true).is_err());
    assert!(!runner.loaded.get());
}
