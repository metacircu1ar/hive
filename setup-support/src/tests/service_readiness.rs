//! Service readiness regressions.
use super::*;

#[test]
fn startup_failures_distinguish_transient_and_suppressed_before_readiness() {
    let (_temp, _layout, _scope, spec) = fixture();
    assert!(!crate::managed::startup_failed(&spec, "recovery_required", false).unwrap());
    let transient = crate::managed::failure(&spec).unwrap().unwrap();
    assert!(!transient.suppressed);
    assert_eq!(transient.code, "recovery_required");
    assert!(crate::managed::startup_failed(&spec, "config_invalid", true).unwrap());
    assert!(crate::managed::failure(&spec).unwrap().unwrap().suppressed);
    assert_eq!(
        crate::managed::failure(&spec).unwrap().unwrap().code,
        "config_invalid"
    );
    crate::managed::clear_failure(&spec).unwrap();
    crate::managed::ready();
    assert!(!crate::managed::startup_failed(&spec, "config_invalid", true).unwrap());
    assert!(crate::managed::failure(&spec).unwrap().is_none());
}

#[test]
fn loaded_but_unready_is_restored_and_restart_errors_remain_typed() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    mock.loaded.set(true);
    ctl.paused(&|| Ok(mock.ready.get()), || Ok(())).unwrap();
    assert!(mock.ready.get());
    for fail_operation in [false, true] {
        mock.fail_start.set(false);
        ctl.start(&|| Ok(mock.ready.get())).unwrap();
        mock.fail_start.set(true);
        let error = ctl
            .paused(&|| Ok(mock.ready.get()), || {
                if fail_operation {
                    Err(crate::invalid("operation failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<service::RestartFailure>()
            .unwrap();
        assert_eq!(failure.operation.is_some(), fail_operation);
        assert!(failure.restart.to_string().contains("mock manager failure"));
    }
}

#[test]
fn controller_keeps_waiting_after_transient_startup_failure() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    let calls = Cell::new(0);
    ctl.start(&|| {
        let n = calls.get();
        calls.set(n + 1);
        if n == 1 {
            files::write(
                &spec.descriptor.with_extension("failure.json"),
                &files::encode(&crate::managed::Failure {
                    code: "io_error".into(),
                    suppressed: false,
                })?,
                true,
            )?;
        }
        Ok(n >= 2)
    })
    .unwrap();
    assert_eq!(calls.get(), 3);
}
