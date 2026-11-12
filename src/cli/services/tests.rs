use super::*;
use crate::calls::ObservationError;
use clap::Parser;
use serde_json::json;

#[test]
fn m5b_explicit_service_paths_fail_before_manager_discovery() {
    let cli = Cli::try_parse_from(["hivectl", "service", "status"]).unwrap();
    for (command, field) in [
        (
            service::Command::Install {
                program: Some("/missing/PRIVATE".into()),
                environment_file: None,
                service_path: None,
            },
            "--program",
        ),
        (
            service::Command::Install {
                program: None,
                environment_file: Some("/missing/PRIVATE".into()),
                service_path: None,
            },
            "--environment-file",
        ),
        (
            service::Command::Install {
                program: None,
                environment_file: None,
                service_path: Some(String::new()),
            },
            "--service-path",
        ),
    ] {
        let error = run(&cli, Path::new("/unused"), Path::new("/unused"), &command)
            .err()
            .unwrap();
        assert_eq!(
            error.code(),
            if field == "--service-path" {
                "invalid_value"
            } else {
                "invalid_path"
            }
        );
        let value = api::value(&error);
        assert_eq!(value["field"], field);
        assert!(!value.to_string().contains("PRIVATE"));
    }
}

#[test]
fn daemon_health_failure_codes_survive_service_adapters() {
    for (code, message) in [
        ("daemon_busy", "Busy."),
        ("provider_protocol_error", "Invalid response."),
        ("observation_deadline", "Observation timed out."),
    ] {
        let typed: ObservationError =
            serde_json::from_value(json!({"code":code,"message":message})).unwrap();
        let observed = observation_error(typed);
        assert_eq!(
            observed.kind(),
            if code == "observation_deadline" {
                std::io::ErrorKind::TimedOut
            } else {
                std::io::ErrorKind::Other
            }
        );
        assert!(
            observed
                .get_ref()
                .unwrap()
                .downcast_ref::<ObservationError>()
                .is_some()
        );
        let error = error(observed);
        assert_eq!(error.code(), code);
        assert_eq!(error.message(), message);
    }
}
