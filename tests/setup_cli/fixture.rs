//! Shared isolated setup/daemon fixture.
use super::*;

pub(super) struct Fixture {
    pub(super) home: tempfile::TempDir,
    pub(super) config: PathBuf,
    pub(super) state: PathBuf,
    pub(super) codex: PathBuf,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let base = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cache/lx-setup");
        hive_setup_support::files::directory(&base).unwrap();
        let home = tempfile::Builder::new()
            .prefix("t")
            .tempdir_in(base)
            .unwrap();
        let root = home.path();
        let scenario = root.join("scenario.json");
        fs::write(&scenario, "{}").unwrap();
        fs::write(
            root.join("mock_common.py"),
            include_str!("../fixtures/mock_common.py"),
        )
        .unwrap();
        support::mock_files(root);
        let codex = root.join("codex");
        fs::write(
            &codex,
            format!(
                "{}SCENARIO = {}\nPROVIDER = 'codex'\n{}",
                support::python_shebang(root),
                json!(scenario),
                include_str!("../fixtures/provider.py")
            ),
        )
        .unwrap();
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            config: root.join("config.yaml"),
            state: root.join("state"),
            home,
            codex,
        }
    }
    pub(super) fn product(&self, scope: &str) -> Command {
        let mut c = support::command(env!("CARGO_BIN_EXE_hivectl"));
        self.environment(&mut c);
        c.arg("--state-dir").arg(&self.state);
        if scope != "remote" {
            c.arg("--config").arg(&self.config);
        }
        if scope == "provider" {
            c.arg("--codex-bin").arg(&self.codex);
        }
        c
    }
    pub(super) fn command(&self) -> Command {
        let mut c = self.product("provider");
        c.arg("setup");
        c
    }
    pub(super) fn removal(&self) -> Command {
        let mut c = support::command(env!("CARGO_BIN_EXE_hivectl"));
        self.environment(&mut c);
        c.args(["setup", "--uninstall", "--yes", "--prefix"])
            .arg(self.home.path().join("installation"));
        c
    }
    pub(super) fn start(&self) -> support::ChildGuard {
        use std::{
            process::Stdio,
            time::{Duration, Instant},
        };
        let mut daemon = support::ChildGuard::new(
            self.product("provider")
                .args(["daemon", "run"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let (out, _) = output(self.product("control").args(["daemon", "status"]));
            if out.status.success() {
                return daemon;
            }
            assert!(daemon.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    pub(super) fn environment(&self, c: &mut Command) {
        c.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("cfg"))
            .env("XDG_STATE_HOME", self.home.path().join("st"))
            .env("XDG_DATA_HOME", self.home.path().join("data"));
    }
    pub(super) fn pty(&self, args: &[&str], answers: Value) -> Output {
        let base = self.command();
        let mut c = support::command(support::python());
        self.environment(&mut c);
        c.env("HIVE_SETUP_ANSWERS", answers.to_string())
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_setup.py"))
            .arg(base.get_program())
            .args(base.get_args())
            .args(args)
            .output()
            .unwrap()
    }
}
pub(super) fn output(c: &mut Command) -> (Output, Value) {
    let out = c.output().unwrap();
    let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, value)
}
pub(super) fn output_json(c: &mut Command) -> (Output, Value) {
    output(c)
}
