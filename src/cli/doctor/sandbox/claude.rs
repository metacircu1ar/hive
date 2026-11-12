//! Keep the Claude prerequisite diagnostic distinct from Codex's released check.
use super::{
    CONTROL_REPLY_GRACE, Cli, Config, Detail, Outcome, Path, Report, Status, api, native_sandbox,
    runtime,
};
use crate::config::Provider;

pub(super) fn check(cli: &Cli, root: &Path, config: &Config, report: &mut Report) {
    if !native_sandbox::configured_for(config, Provider::Claude) {
        return;
    }
    let response = crate::calls::executor()
        .map_err(crate::calls::ObservationError::from)
        .and_then(|executor| {
            executor.block_on(async {
                runtime::client::Client::new(root)
                    .observe::<native_sandbox::Report>(
                        "doctor-claude-sandbox-probe",
                        &crate::runtime::api::Request::ClaudeSandboxProbe,
                        tokio::time::Instant::now() + native_sandbox::TIMEOUT + CONTROL_REPLY_GRACE,
                    )
                    .await
            })
        })
        .inspect_err(|error| {
            tracing::warn!(name: "sandbox.daemon_unverified",
                code = error.code().as_str(), decision = "check_caller_separately");
        });
    if let Ok(diagnostic) = response {
        add(report, "daemon", diagnostic);
    } else {
        report.add(
            "claude_sandbox:daemon",
            Status::Warning,
            api::value("Daemon-context Claude sandbox prerequisites are unverified."),
            vec!["Start or update the Hive daemon, then run doctor again.".into()],
        );
        let diagnostic = match crate::process::executable(&cli.claude_bin) {
            Ok(binary) => native_sandbox::run_for(&binary, Provider::Claude),
            Err(error) => {
                tracing::warn!(name: "sandbox.provider_unavailable",
                        phase = "caller_local", code = error.code(),
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
    let fixes = match diagnostic.outcome {
        Outcome::Failed => vec![
            "Check the native Claude helper and Bubblewrap in the daemon's launch context. \
            On Ubuntu inspect AppArmor unpriv_bwrap capability denials; see the setup guide's \
            Claude sandbox prerequisites. Do not disable Unix-socket filtering or silently \
            retry unsandboxed."
                .into(),
        ],
        Outcome::Unavailable => vec![
            "Install or repair the configured Claude executable, then restart the Hive daemon."
                .into(),
        ],
        _ => vec![],
    };
    report.add(
        format!("claude_sandbox:{context}"),
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

    fn config() -> Config {
        Config::parse(
            "auth_profiles: {main: {provider: claude, method: subscription}}\n\
            default_provider: claude\nproviders: {claude: {model: default}}\n",
        )
        .unwrap()
    }

    #[test]
    fn claude_failure_uses_daemon_binary_and_separate_operation() {
        let stub = Stub::new(vec![
            json!({"outcome":"failed","executable":"/daemon/claude","code":"sandbox_permission_denied"}),
        ]);
        let cli = Cli::try_parse_from(["hivectl", "--claude-bin", "/missing", "doctor"]).unwrap();
        let mut report = Report::default();
        super::super::check(&cli, stub.root(), &config(), &mut report);
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.checks[0].name, "claude_sandbox:daemon");
        assert_eq!(
            report.checks[0].detail["diagnostic"]["executable"],
            "/daemon/claude"
        );
        assert!(!report.finish().success);
        assert_eq!(
            stub.finish()[0]["args"],
            json!({"op":"claude_sandbox_probe"})
        );
    }

    #[test]
    fn old_daemon_does_not_imply_claude_readiness() {
        let stub = Stub::new(vec![
            json!({"error":{"code":"unsupported_operation","message":"unsupported"}}),
        ]);
        let cli = Cli::try_parse_from(["hivectl", "--claude-bin", "/missing", "doctor"]).unwrap();
        let mut report = Report::default();
        check(&cli, stub.root(), &config(), &mut report);
        assert_eq!(report.checks.len(), 2);
        assert!(report.checks[0].status == Status::Warning);
        assert_eq!(report.checks[1].name, "claude_sandbox:caller_local");
        assert!(report.checks[1].status == Status::Error);
        stub.finish();
    }
}
