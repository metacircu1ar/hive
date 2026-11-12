//! Service ownership regressions.
use super::*;

#[test]
fn newer_service_descriptor_refuses_control_without_touching_files_or_manager() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let controller = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    controller.install(&spec).unwrap();
    let unit = fs::read(&spec.unit).unwrap();
    let mut value = serde_json::to_value(&spec).unwrap();
    value["format"] = 2.into();
    value["new_field"] = true.into();
    files::write(&spec.descriptor, &serde_json::to_vec(&value).unwrap(), true).unwrap();
    let before = fs::read(&spec.descriptor).unwrap();
    mock.calls.borrow_mut().clear();
    for error in [
        service::load(&layout, &scope).err().unwrap(),
        controller.stop(&|| Ok(false)).unwrap_err(),
        controller.uninstall(&|| Ok(false)).unwrap_err(),
    ] {
        assert!(hive_input::compatibility::record::is_newer(&error));
    }
    assert!(mock.calls.borrow().is_empty());
    assert_eq!(fs::read(&spec.descriptor).unwrap(), before);
    assert_eq!(fs::read(&spec.unit).unwrap(), unit);
}

#[test]
fn foreign_jobs_and_edited_units_are_never_controlled() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    mock.loaded.set(true);
    assert!(ctl.install(&spec).is_err());
    assert!(!spec.descriptor.exists());
    mock.loaded.set(false);
    ctl.install(&spec).unwrap();
    files::write(&spec.unit, b"foreign", true).unwrap();
    mock.calls.borrow_mut().clear();
    assert!(ctl.stop(&|| Ok(false)).is_err());
    assert!(ctl.uninstall(&|| Ok(false)).is_err());
    assert!(mock.calls.borrow().is_empty());
    assert_eq!(fs::read(&spec.unit).unwrap(), b"foreign");
}

#[test]
fn unavailable_manager_is_visible_and_installs_nothing() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    mock.unavailable.set(true);
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    let status = ctl.status(&|| Ok(false)).unwrap();
    assert!(!status.manager_available);
    assert!(!status.managed);
    assert!(ctl.install(&spec).is_err());
    assert!(!spec.descriptor.exists());
    assert!(!spec.unit.exists());
}

#[test]
fn setup_refuses_a_service_created_after_its_absence_observation() {
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    assert!(!ctl.status(&|| Ok(false)).unwrap().managed);
    ctl.install(&spec).unwrap(); // Other setup wins the observation/install race.
    assert!(ctl.install_new(&spec).is_err());
    assert!(!runner.loaded.get());
}
