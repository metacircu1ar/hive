//! Real CLI preparation shares prompts and refusals with ordinary setup.
use super::*;
use hive_setup_support::preparation::{self, Resources};
use std::os::unix::fs::symlink;

struct Prepared {
    directory: tempfile::TempDir,
    resources: PathBuf,
    answers: PathBuf,
    prefix: PathBuf,
}
impl Prepared {
    fn new(f: &Fixture) -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(f.home.path())
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        let resources = root.join("resources.json");
        let prefix = f.home.path().canonicalize().unwrap().join("installation");
        preparation::write(
            &resources,
            &Resources {
                format: hive_input::compatibility::Exact,
                prefix: prefix.clone(),
                hive_source: Some(root.clone()),
                comb_source: None,
                llama_source: None,
            },
        )
        .unwrap();
        Self {
            directory,
            resources,
            answers: root.join("answers.json"),
            prefix,
        }
    }
    fn prepare(&self, f: &Fixture) -> Command {
        let mut command = f.command();
        command
            .arg("--prepare-resources")
            .arg(&self.resources)
            .arg("--prepare-output")
            .arg(&self.answers);
        command
    }
    fn apply(&self, f: &Fixture) -> Command {
        fs::create_dir_all(self.prefix.join("bin")).unwrap();
        let binary = self.prefix.join("bin/hivectl");
        symlink(env!("CARGO_BIN_EXE_hivectl"), &binary).unwrap();
        let original = f.product("provider");
        let mut command = support::command(&binary);
        f.environment(&mut command);
        let checksum = preparation::digest(&fs::read(&self.answers).unwrap());
        command
            .args(original.get_args())
            .arg("setup")
            .arg("--apply-prepared")
            .arg(&self.answers)
            .arg("--prepared-sha256")
            .arg(checksum);
        command
    }
}
const CHOICES: &[&str] = &[
    "--yes",
    "--provider",
    "codex",
    "--login",
    "skip",
    "--codex-model",
    "fixture-default",
    "--codex-effort",
    "low",
    "--no-comb",
    "--no-services",
];

#[test]
fn prepared_setup_requires_complete_answers_before_any_mutation() {
    let f = Fixture::new();
    let p = Prepared::new(&f);
    let (out, value) = output(p.prepare(&f).arg("--yes"));
    assert_eq!(out.status.code(), Some(2), "{value}");
    assert_eq!(value["error"]["field"], "--provider");
    assert!(!p.prefix.exists());
    assert!(!p.answers.exists());
    assert!(!f.config.exists());
    assert!(!f.state.exists());
    assert!(p.directory.path().exists());
}

#[test]
fn prepared_setup_asks_once_and_applies_saved_answers_without_original_file() {
    let f = Fixture::new();
    let p = Prepared::new(&f);
    let original = f.home.path().join("choices.yaml");
    fs::write(
        &original,
        "codex-model: fixture-default\ncodex-effort: low\nno-comb: true\nno-services: true\n",
    )
    .unwrap();
    fs::set_permissions(&original, fs::Permissions::from_mode(0o600)).unwrap();
    let out = f.pty(
        &[
            "--answers",
            original.to_str().unwrap(),
            "--prepare-resources",
            p.resources.to_str().unwrap(),
            "--prepare-output",
            p.answers.to_str().unwrap(),
        ],
        json!([["(--provider)", "codex"], ["(--login)", "skip"]]),
    );
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.matches("(--provider)").count(), 1);
    assert_eq!(text.matches("(--login)").count(), 1);
    assert!(!p.prefix.exists());
    assert!(!f.config.exists());
    assert!(!f.state.exists());
    fs::remove_file(&original).unwrap();
    let (out, value) = output(p.apply(&f).arg("--answers").arg(&original));
    assert!(out.status.success(), "{value}");
    assert_eq!(value["configured"], true);
    assert!(f.config.exists());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("(--provider)"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("(--login)"));
}

#[test]
fn prepared_setup_revalidates_changed_configuration_instead_of_replaying_a_plan() {
    let f = Fixture::new();
    let p = Prepared::new(&f);
    let (out, value) = output(p.prepare(&f).args(CHOICES));
    assert!(out.status.success(), "{value}");
    let changed = concat!(
        "auth_profiles:\n  codex-main: {provider: codex, method: subscription}\n",
        "providers:\n  codex: {auth: codex-main, model: changed, effort: low}\n"
    );
    fs::write(&f.config, changed).unwrap();
    let (out, _) = output(&mut p.apply(&f));
    assert!(!out.status.success());
    assert_eq!(fs::read_to_string(&f.config).unwrap(), changed);
    assert!(!f.state.exists());
}

#[test]
fn prepared_setup_does_not_skip_conflicts_after_interactive_default_choice() {
    for device in [true, false] {
        let f = Fixture::new();
        let p = Prepared::new(&f);
        let config = concat!(
            "auth_profiles:\n  codex-main: {provider: codex, method: subscription}\n",
            "  claude-main: {provider: claude, method: subscription}\n",
            "providers:\n  codex: {auth: codex-main, model: existing-model, effort: low}\n"
        );
        fs::write(&f.config, config).unwrap();
        let extra = if device {
            ["--login", "device"]
        } else {
            ["--codex-model", "different-model"]
        };
        let out = f.pty(
            &[
                "--prepare-resources",
                p.resources.to_str().unwrap(),
                "--prepare-output",
                p.answers.to_str().unwrap(),
                "--no-comb",
                "--no-services",
                extra[0],
                extra[1],
            ],
            json!([["(--default-provider)", "codex"]]),
        );
        assert_eq!(
            out.status.code(),
            Some(if device { 2 } else { 1 }),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!p.answers.exists());
        assert!(!p.prefix.exists());
        assert!(!f.state.exists());
        assert_eq!(fs::read_to_string(&f.config).unwrap(), config);
    }
}

#[test]
fn prepared_setup_refuses_user_edited_skills_before_activation() {
    let f = Fixture::new();
    let p = Prepared::new(&f);
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills");
    let (out, result) = output(
        f.command()
            .args(CHOICES)
            .args(["--user-skills", "codex"])
            .arg("--skills-dir")
            .arg(&source),
    );
    assert!(out.status.success(), "{result}");
    let config = fs::read(&f.config).unwrap();
    let guide = f.home.path().join(".codex/skills/hive/SKILL.md");
    fs::write(&guide, "User-edited guide").unwrap();
    let (out, _) = output(
        p.prepare(&f)
            .args(CHOICES)
            .args(["--user-skills", "codex"])
            .arg("--skills-dir")
            .arg(&source),
    );
    assert!(!out.status.success());
    assert!(!p.answers.exists());
    assert!(!p.prefix.exists());
    assert_eq!(fs::read(&f.config).unwrap(), config);
    assert_eq!(fs::read_to_string(&guide).unwrap(), "User-edited guide");
}

#[test]
#[ignore = "Actual setup/login chain fixture driven through an isolated pseudo-terminal"]
fn native_login_cancellation_child() {
    use std::{io::Error, os::unix::fs::OpenOptionsExt};
    let f = Fixture::new();
    fs::write(
        f.home.path().join("scenario.json"),
        json!({"login_ignore_signals": true}).to_string(),
    )
    .unwrap();
    let pid_file = f.home.path().join("native.pid");
    let original = fs::read_to_string(&f.codex).unwrap().replace(
        "    print(\"fake-login: enter confirmation\", flush=True)",
        &format!(
            "    pathlib.Path({}).write_text(str(os.getpid()))\n    \
             print(\"fake-login: enter confirmation\", flush=True)",
            json!(pid_file)
        ),
    );
    let (shebang, body) = original.split_once('\n').unwrap();
    fs::write(
        &f.codex,
        format!(
            "{shebang}\nimport os,pathlib,signal,sys\nif 'login' in sys.argv:\n    \
         signal.signal(signal.SIGHUP,signal.SIG_IGN)\n{body}"
        ),
    )
    .unwrap();
    let terminal = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/tty")
        .unwrap();
    let mut command = f.command();
    command
        .args([
            "--provider",
            "codex",
            "--login",
            "browser",
            "--codex-model",
            "fixture-default",
            "--codex-effort",
            "low",
            "--no-comb",
            "--no-services",
        ])
        .stdin(terminal.try_clone().unwrap())
        .stdout(terminal.try_clone().unwrap())
        .stderr(terminal.try_clone().unwrap());
    let error = hive_setup_support::runner::attached_command(
        &mut command,
        Some(&terminal),
        std::time::Duration::from_secs(60),
        // Model cancellation arriving at the installer while the real login
        // owner is active. The marker is written after the provider ignores
        // INT/TERM, so only that owner's full escalation can make it disappear.
        &|| pid_file.exists(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    let pid: libc::pid_t = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    // SAFETY: signal zero observes only; it never mutates an unowned or reused PID.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    println!("NATIVE_LOGIN_CANCELLED");
}

#[test]
fn native_login_owner_gets_cancellation_and_reaps_an_uncooperative_provider() {
    let mut command = support::command(support::python());
    let out = command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_setup.py"))
        .arg(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "prepared::native_login_cancellation_child",
            "--nocapture",
        ])
        .env("HIVE_SETUP_ANSWERS", json!([]).to_string())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("NATIVE_LOGIN_CANCELLED"), "{text}");
    assert!(text.contains("Interactive setup step failed"), "{text}");
    assert!(text.contains("SETUP_PIPE_PRESERVED"), "{text}");
}
