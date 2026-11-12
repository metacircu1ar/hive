//! Service publication regressions.
use super::*;

#[test]
fn installing_services_does_not_create_the_default_hive_data_root() {
    for (product, custom) in [("hive", false), ("hive", true), ("comb", false)] {
        let (_temp, layout, _, _) = fixture();
        let default_state = layout.state.join("hive");
        let config = layout.config.join("hive/config.yaml");
        let root = match (product, custom) {
            ("hive", false) => default_state.clone(),
            ("hive", true) => layout.home.join("custom-state"),
            _ => layout.home.join("Comb"),
        };
        let paths: Vec<&std::path::Path> = if product == "hive" {
            vec![&config, &root]
        } else {
            vec![&root]
        };
        let scope = Scope::new(product, &paths).unwrap();
        let spec = Spec::new(
            &layout,
            scope.clone(),
            crate::service::Program {
                path: std::path::Path::new("/bin/echo"),
                arguments: vec![],
                environment_file: None,
                required_environment: vec![],
            },
        )
        .unwrap();
        let runner = Mock::new();
        let controller = Controller {
            layout: &layout,
            scope: &scope,
            runner: &runner,
        };
        controller.install_new(&spec).unwrap();
        assert!(spec.log.is_file());
        assert!(!default_state.exists());
        assert!(!root.exists());
        spec.verify(&layout, &scope).unwrap();
        assert_eq!(service::load(&layout, &scope).unwrap(), Some(spec.clone()));
        controller.install(&spec).unwrap();
        assert!(!default_state.exists());
        assert!(!root.exists());
        spec.verify(&layout, &scope).unwrap();
    }
}

#[test]
fn previous_service_log_layout_is_refused_without_adoption() {
    let (_temp, layout, scope, mut spec) = fixture();
    spec.log = layout
        .state
        .join("hive/logs")
        .join(format!("{}.log", scope.label().unwrap()));
    spec = spec.rendered().unwrap();
    let descriptor = files::encode(&spec).unwrap();
    files::write(&spec.descriptor, &descriptor, false).unwrap();
    files::write(&spec.unit, spec.unit_contents.as_bytes(), false).unwrap();
    assert!(service::load(&layout, &scope).is_err());
    assert_eq!(files::read(&spec.descriptor).unwrap().unwrap(), descriptor);
    assert_eq!(
        files::read(&spec.unit).unwrap().unwrap(),
        spec.unit_contents.as_bytes()
    );
    assert!(!spec.log.exists());
    assert!(!layout.log(&scope).unwrap().exists());
}

#[test]
fn identical_record_retry_repeats_a_failed_publication_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record");
    files::FAIL_SYNC.with(|fault| *fault.borrow_mut() = Some(path.clone()));
    assert!(files::write(&path, b"published", false).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"published");
    // An identical retry cannot claim success while the barrier still fails.
    assert!(files::write(&path, b"published", false).is_err());
    files::FAIL_SYNC.with(|fault| *fault.borrow_mut() = None);
    files::SYNCED.with(|paths| paths.borrow_mut().clear());
    files::write(&path, b"published", false).unwrap();
    assert!(files::SYNCED.with(|paths| paths.borrow().contains(&path)));
}

#[test]
fn interrupted_install_completes_exact_owned_descriptor_only() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    files::write(&spec.descriptor, &files::encode(&spec).unwrap(), false).unwrap();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    assert!(service::load(&layout, &scope).unwrap().is_some());
}

#[test]
fn unit_refresh_recovers_between_unit_and_descriptor_publication() {
    #[derive(serde::Serialize)]
    struct RefreshFixture<'a> {
        old: &'a Spec,
        next: &'a Spec,
    }
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    ctl.install(&spec).unwrap();
    let mut old = spec.clone();
    old.unit_contents.push_str("\n<!-- old -->\n");
    files::write(&old.descriptor, &files::encode(&old).unwrap(), true).unwrap();
    // Descriptor is old, unit already new: exact journal authorizes recovery.
    files::write(
        &spec.descriptor.with_extension("refresh.json"),
        &files::encode(&RefreshFixture {
            old: &old,
            next: &spec,
        })
        .unwrap(),
        false,
    )
    .unwrap();
    ctl.install(&spec).unwrap();
    spec.verify(&layout, &scope).unwrap();
    assert!(!spec.descriptor.with_extension("refresh.json").exists());
}
