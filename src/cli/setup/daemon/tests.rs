use super::*;
use hive_setup_support::{
    files,
    runner::{Output, Runner},
    service::{Platform, Program, Scope, Spec},
};
use std::{cell::Cell, fs};

struct Absent(Cell<usize>);
impl Runner for Absent {
    fn run(&self, program: &str, arguments: &[String]) -> std::io::Result<Output> {
        assert_eq!(program, "/bin/launchctl");
        assert_eq!(arguments[0], "print-disabled");
        self.0.set(self.0.get() + 1);
        Ok(Output {
            code: 112,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}
#[test]
fn shared_setup_plan_refuses_owned_service_changes_and_malformed_failure_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let layout = Layout {
        home: home.clone(),
        config: home.join("cfg"),
        state: home.join("st"),
        platform: Platform::Launchd,
    };
    let config = home.join("config.yaml");
    let root = home.join("state");
    let environment = home.join("service.env");
    files::write(&environment, b"", false).unwrap();
    let spec = Spec::new(
        &layout,
        Scope::new("hive", &[&config, &root]).unwrap(),
        Program {
            path: Path::new("/bin/sh"),
            arguments: vec![],
            environment_file: Some(&environment),
            required_environment: vec![],
        },
    )
    .unwrap();
    files::write(&spec.descriptor, &files::encode(&spec).unwrap(), false).unwrap();
    files::write(&spec.unit, &spec.render().unwrap(), false).unwrap();
    let original = [
        fs::read(&spec.descriptor).unwrap(),
        fs::read(&spec.unit).unwrap(),
    ];
    let runner = Absent(Cell::new(0));
    for options in [
        Values {
            service_environment: Some(home.join("different.env")),
            ..Values::default()
        },
        Values {
            service_path: Some("/different/bin".into()),
            ..Values::default()
        },
    ] {
        assert!(preflight_with(&config, &root, &options, &layout, &runner).is_err());
    }
    let failure = spec.descriptor.with_extension("failure.json");
    files::write(&failure, b"{\"format\":1,\"unknown\":true}", false).unwrap();
    assert!(preflight_with(&config, &root, &Values::default(), &layout, &runner).is_err());
    assert_eq!(fs::read(&spec.descriptor).unwrap(), original[0]);
    assert_eq!(fs::read(&spec.unit).unwrap(), original[1]);
    assert!(!root.exists());
    assert!(runner.0.get() >= 3);
}
