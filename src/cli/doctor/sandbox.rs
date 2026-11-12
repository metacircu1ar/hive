//! Native execution health must be tested in the daemon's launch context.
use super::{Cli, Config, Path, Report, Status, api, runtime};
use crate::native_sandbox::{self, Outcome};
use serde::Serialize;
mod claude;

const CONTROL_REPLY_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Serialize)]
struct Detail<'a> {
    context: &'a str,
    diagnostic: native_sandbox::Report,
}

pub(super) fn check(cli: &Cli, root: &Path, config: &Config, report: &mut Report) {
    check_codex(cli, root, config, report);
    claude::check(cli, root, config, report);
}

fn check_codex(cli: &Cli, root: &Path, config: &Config, report: &mut Report) {
    if !native_sandbox::configured(config) {
        return;
    }
    let response = crate::calls::executor()
        .map_err(crate::calls::ObservationError::from)
        .and_then(|executor| {
            executor.block_on(async {
                runtime::client::Client::new(root)
                    .observe::<native_sandbox::Report>(
                        "doctor-sandbox-probe",
                        &super::super::Request::SandboxProbe,
                        tokio::time::Instant::now() + native_sandbox::TIMEOUT + CONTROL_REPLY_GRACE,
                    )
                    .await
            })
        })
        .inspect_err(|error| {
            tracing::warn!(name: "sandbox.daemon_unverified",
                 code = error.code().as_str(),
                 decision = "check_caller_separately");
        });
    if let Ok(diagnostic) = response {
        add(report, "daemon", diagnostic);
    } else {
        report.add(
            "native_sandbox:daemon",
            Status::Warning,
            api::value(
                "Daemon-context sandbox execution is unverified: the daemon is \
            unavailable or cannot provide the diagnostic.",
            ),
            vec!["Start or update the Hive daemon, then run doctor again.".into()],
        );
        let diagnostic = match crate::process::runtime_codex_executable(&cli.codex_bin) {
            Ok(binary) => native_sandbox::run(&binary),
            Err(error) => {
                tracing::warn!(name: "sandbox.provider_unavailable",
                     phase = "caller_local",
                     code = error.code(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "return_unavailable_diagnostic");
                native_sandbox::Report::new(Outcome::Unavailable, None, "provider_unavailable")
            }
        };
        add(report, "caller_local", diagnostic);
    }
}

fn add(report: &mut Report, context: &str, diagnostic: native_sandbox::Report) {
    let status = match diagnostic.outcome {
        Outcome::Passed => Status::Ok,
        Outcome::Failed | Outcome::Unavailable => Status::Error,
        Outcome::Busy | Outcome::Unsupported | Outcome::NotConfigured => Status::Warning,
    };
    let fixes = if diagnostic.outcome == Outcome::Failed {
        vec![
            "Check native Codex sandbox execution from the daemon's launch context. On Ubuntu, \
            inspect AppArmor denials and the distribution's Bubblewrap profile; see \
            https://learn.chatgpt.com/docs/sandboxing."
                .into(),
        ]
    } else if diagnostic.outcome == Outcome::Unavailable {
        vec![
            "Install or repair the configured Codex executable, then restart the Hive daemon \
            so it resolves the executable again."
                .into(),
        ]
    } else {
        vec![]
    };
    report.add(
        format!("native_sandbox:{context}"),
        status,
        api::value(Detail {
            context,
            diagnostic,
        }),
        fixes,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::response_tests::Stub;
    use clap::Parser;
    use serde_json::json;

    fn codex_config() -> Config {
        Config::parse(
            "auth_profiles: {main: {provider: codex, method: \
            subscription}}\nproviders: {codex: {model: default}}\n",
        )
        .unwrap()
    }

    #[test]
    fn native_sandbox_fallback_records_codes_without_private_messages() {
        const CHILD: &str = "HIVE_SANDBOX_FALLBACK_LOG_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let (_, module) = module_path!().split_once("::").unwrap();
            let mut command = crate::test_support::rerun_isolated(
                &format!(
                    "{module}::native_sandbox_fallback_records_codes_without_private_messages"
                ),
                false,
            );
            crate::test_support::run_isolated(command.env(CHILD, "1"));
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("diagnostics.jsonl");
        let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
        backend.install_global().unwrap();
        let stub = Stub::new(vec![json!({"error":{
            "code":"unsupported_operation", "message":"secret-daemon-probe-canary"
        }})]);
        let cli =
            Cli::try_parse_from(["hivectl", "--codex-bin", "/missing-codex", "doctor"]).unwrap();
        {
            let mut report = Report::default();
            check(&cli, stub.root(), &codex_config(), &mut report);
            assert_eq!(report.checks.len(), 2);
            assert!(report.checks[0].status == Status::Warning);
            assert!(report.checks[1].status == Status::Error);
        }
        stub.finish();
        assert_eq!(
            guard.finish(std::time::Duration::from_secs(2)).state,
            hive_log::Shutdown::Drained
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("secret-daemon-probe-canary"));
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let failures: Vec<_> = rows
            .iter()
            .filter(|row| row["event"] == "sandbox.daemon_unverified")
            .collect();
        assert_eq!(failures.len(), 1, "{text}");
        assert_eq!(failures[0]["code"], "unsupported_operation");
        let failures: Vec<_> = rows
            .iter()
            .filter(|row| row["event"] == "sandbox.provider_unavailable")
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["phase"], "caller_local");
        assert_eq!(failures[0]["code"], "missing_binary");
    }

    #[test]
    fn daemon_failure_is_authoritative_even_when_caller_binary_is_missing() {
        let stub = Stub::new(vec![
            json!({"outcome":"failed","executable":"/daemon/codex","code":"sandbox_permission_denied"}),
        ]);
        let cli =
            Cli::try_parse_from(["hivectl", "--codex-bin", "/no-such-codex", "doctor"]).unwrap();
        let mut report = Report::default();
        check(&cli, stub.root(), &codex_config(), &mut report);
        assert_eq!(report.checks.len(), 1);
        assert_eq!(
            report.checks[0].detail["diagnostic"]["executable"],
            "/daemon/codex"
        );
        assert!(!report.finish().success);
        assert_eq!(stub.finish()[0]["args"], json!({"op":"sandbox_probe"}));
    }

    #[test]
    fn unavailable_diagnostic_never_claims_daemon_readiness() {
        let stub = Stub::new(vec![
            json!({"error":{"code":"unsupported_operation","message":"Unavailable"}}),
        ]);
        let cli =
            Cli::try_parse_from(["hivectl", "--codex-bin", "/no-such-codex", "doctor"]).unwrap();
        let mut report = Report::default();
        check(&cli, stub.root(), &codex_config(), &mut report);
        assert_eq!(report.checks.len(), 2);
        assert_eq!(report.checks[0].name, "native_sandbox:daemon");
        assert!(report.checks[0].status == Status::Warning);
        assert_eq!(report.checks[1].name, "native_sandbox:caller_local");
        assert!(report.checks[1].status == Status::Error);
        stub.finish();
    }

    #[test]
    fn unavailable_daemon_binary_fails_even_with_an_available_caller_binary() {
        let stub = Stub::new(vec![
            json!({"outcome":"unavailable","executable":null,"code":"provider_unavailable"}),
        ]);
        let cli = Cli::try_parse_from(["hivectl", "--codex-bin", "/bin/echo", "doctor"]).unwrap();
        assert!(crate::process::runtime_codex_executable(&cli.codex_bin).is_ok());
        let mut report = Report::default();
        check(&cli, stub.root(), &codex_config(), &mut report);
        assert_eq!(report.checks.len(), 1);
        assert!(report.checks[0].fixes[0].contains("restart"));
        assert!(!report.finish().success);
        stub.finish();
    }

    #[test]
    fn failures_and_unverified_contexts_cannot_be_reported_as_passed() {
        for (outcome, expected) in [
            (Outcome::Passed, Status::Ok),
            (Outcome::Failed, Status::Error),
            (Outcome::Unavailable, Status::Error),
            (Outcome::Busy, Status::Warning),
            (Outcome::Unsupported, Status::Warning),
            (Outcome::NotConfigured, Status::Warning),
        ] {
            let mut report = Report::default();
            add(
                &mut report,
                "daemon",
                native_sandbox::Report::new(outcome, None, "test"),
            );
            assert!(report.checks[0].status == expected);
            assert_eq!(report.checks[0].detail["context"], "daemon");
        }
    }
}
