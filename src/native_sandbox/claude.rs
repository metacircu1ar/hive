//! Linux prerequisites for Claude's bundled native seccomp helper, not a model call.
use super::{MARKER, Outcome, Path, Provider, Report, classify};
use std::{env, io, os::unix::ffi::OsStrExt, process::Command, time::Duration};

pub(super) fn execute_in(binary: &Path, home: &Path, timeout: Duration) -> io::Result<Report> {
    let bwrap =
        crate::process::executable(std::ffi::OsStr::new("bwrap")).map_err(io::Error::other)?;
    let mut command = match probe_command(&bwrap, binary, &home.canonicalize()?) {
        Ok(command) => command,
        Err(report) => return Ok(report),
    };
    let output = hive_setup_support::runner::capture_command(
        crate::os::spawn,
        &mut command,
        timeout,
        &|| false,
    )?;
    Ok(classify(binary, &output))
}

fn probe_command(bwrap: &Path, binary: &Path, home: &Path) -> Result<Command, Report> {
    // env would consume this executable as another assignment and run echo alone.
    if binary.as_os_str().as_bytes().contains(&b'=') {
        tracing::warn!(name: "sandbox.helper_path_unverifiable",
            code = "helper_path_contains_equals", decision = "return_failed_diagnostic");
        return Err(Report::new(
            Outcome::Failed,
            Some(binary.into()),
            "helper_path_contains_equals",
        ));
    }
    let mut command = Command::new(bwrap);
    command
        .env_clear()
        .envs(crate::process::child_environment(
            Provider::Claude,
            home,
            env::vars_os().map(|(key, value)| crate::process::EnvironmentVariable { key, value }),
        ))
        .current_dir(home)
        .args([
            "--unshare-user",
            "--unshare-net",
            "--unshare-pid",
            "--cap-drop",
            "ALL",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--",
            "/usr/bin/env",
            // Native Claude embeds this helper. Older/script distributions may
            // not support it; their failure must never imply sandbox readiness.
            "ARGV0=apply-seccomp",
        ])
        .arg(binary)
        .args(["/bin/echo", MARKER]);
    Ok(command)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn claude_probe_runs_the_bundled_helper_with_no_login_or_model() {
        let directory = tempfile::tempdir().unwrap();
        let bwrap = directory.path().join("bwrap");
        fs::write(
            &bwrap,
            r#"#!/bin/sh
test "$HOME" = "$CLAUDE_CONFIG_DIR" || exit 1
test "$PWD" = "$HOME" || exit 2
test ! -e "$HOME/.credentials.json" || exit 3
test -z "$ANTHROPIC_API_KEY$OPENAI_API_KEY$CLAUDE_CODE_OAUTH_TOKEN" || exit 4
test "$1" = --unshare-user && test "$2" = --unshare-net && test "$3" = --unshare-pid || exit 5
shift 3
test "$1" = --cap-drop && test "$2" = ALL || exit 6
shift 2
test "$1" = --ro-bind && test "$2" = / && test "$3" = / || exit 7
shift 3
test "$1" = --dev && test "$2" = /dev && test "$3" = --proc && test "$4" = /proc || exit 8
shift 4
test "$1" = -- && test "$2" = /usr/bin/env && test "$3" = ARGV0=apply-seccomp || exit 9
shift 3
test "$1" = '/provider/claude with spaces' || exit 10
shift
test "$#" = 2 && test "$1" = /bin/echo || exit 11
exec "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&bwrap, fs::Permissions::from_mode(0o700)).unwrap();
        let home = super::super::probe_directory().unwrap();
        let binary = Path::new("/provider/claude with spaces");
        let mut command =
            probe_command(&bwrap, binary, &home.path().canonicalize().unwrap()).unwrap();
        let output = hive_setup_support::runner::capture_command(
            crate::os::spawn,
            &mut command,
            super::super::TIMEOUT,
            &|| false,
        )
        .unwrap();
        assert_eq!(
            classify(binary, &output).outcome,
            super::super::Outcome::Passed
        );
    }

    #[test]
    fn an_assignment_path_cannot_skip_the_claude_helper() {
        let directory = tempfile::tempdir().unwrap();
        let bwrap = directory.path().join("bwrap");
        // Execute the real env command: skipping it would hide assignment parsing.
        script(
            &bwrap,
            "while [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"\n",
        );
        let home = super::super::probe_directory().unwrap();
        for name in ["claude with spaces", "claude=assignment"] {
            let binary = directory.path().join(name);
            script(
                &binary,
                "test \"$ARGV0\" = apply-seccomp || exit 71\nexec \"$@\"\n",
            );
            let report = match probe_command(&bwrap, &binary, &home.path().canonicalize().unwrap())
            {
                Ok(mut command) => {
                    let output = hive_setup_support::runner::capture_command(
                        crate::os::spawn,
                        &mut command,
                        super::super::TIMEOUT,
                        &|| false,
                    )
                    .unwrap();
                    classify(&binary, &output)
                }
                Err(report) => report,
            };
            if name.contains('=') {
                assert_eq!(report.outcome, Outcome::Failed);
                assert_eq!(report.code, "helper_path_contains_equals");
            } else {
                assert_eq!(report.outcome, Outcome::Passed);
            }
        }
    }

    #[expect(
        clippy::unwrap_used,
        reason = "The fixture script writer must fail on setup errors."
    )]
    fn script(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}
