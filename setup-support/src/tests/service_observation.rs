use super::*;

#[test]
fn inspection_admits_all_owned_metadata_without_claiming_or_probing_readiness() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let controller = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    controller.install(&spec).unwrap();
    let observed = controller.inspect().unwrap();
    assert!(observed.managed);
    assert!(observed.manager_available);
    let calls = Cell::new(0);
    let status = controller
        .status(&|| {
            calls.set(calls.get() + 1);
            Ok(false)
        })
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(
        serde_json::to_value(observed.with_readiness(false)).unwrap(),
        serde_json::to_value(status).unwrap()
    );
    assert!(controller.inspect().unwrap().with_readiness(true).ready);
}

#[test]
fn malformed_failure_metadata_refuses_inspection_and_status_before_health() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let controller = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    controller.install(&spec).unwrap();
    let unit = fs::read(&spec.unit).unwrap();
    let descriptor = fs::read(&spec.descriptor).unwrap();
    let failure = spec.descriptor.with_extension("failure.json");
    for bytes in [
        b"{\"code\":\"invalid code\",\"suppressed\":true}".as_slice(),
        b"{\"code\":\"startup_error\",\"suppressed\":true,\"unknown\":1}",
    ] {
        files::write(&failure, bytes, true).unwrap();
        assert!(controller.inspect().is_err());
        let calls = Cell::new(0);
        assert!(
            controller
                .status(&|| {
                    calls.set(calls.get() + 1);
                    Ok(false)
                })
                .is_err()
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(fs::read(&failure).unwrap(), bytes);
        assert_eq!(fs::read(&spec.unit).unwrap(), unit);
        assert_eq!(fs::read(&spec.descriptor).unwrap(), descriptor);
    }
    assert!(!mock.loaded.get());
}
