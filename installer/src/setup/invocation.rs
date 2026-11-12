//! Select terminal/argv policy once and retain it across prepare and apply.
use crate::{Result, options::Args};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

pub(super) struct Invocation {
    arguments: Vec<OsString>,
    terminal: Option<File>,
}
impl Invocation {
    pub(super) fn new(args: &Args) -> Self {
        let terminal = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::info!(name: "install.setup_noninteractive",
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "use_null_input");
                None
            }
        };
        let mut arguments = args.setup.clone();
        if !arguments.iter().any(|arg| {
            arg.to_string_lossy()
                .starts_with("--preserve-stopped-services")
        }) {
            arguments.push("--preserve-stopped-services".into());
        }
        if (args.yes || terminal.is_none())
            && !arguments
                .iter()
                .any(|arg| arg == "--yes" || arg.to_string_lossy().starts_with("--yes="))
        {
            arguments.push("--yes".into());
        }
        Self {
            arguments,
            terminal,
        }
    }
    pub(super) fn help(&self) -> bool {
        self.arguments
            .iter()
            .any(|arg| arg == "--help" || arg == "-h")
    }
    pub(super) fn prepare(&self, program: &Path, resources: &Path, output: &Path) -> Result<u8> {
        let mut command = self.command(program);
        command
            .arg("--prepare-resources")
            .arg(resources)
            .arg("--prepare-output")
            .arg(output);
        self.run(command)
    }
    pub(super) fn apply(&self, program: &Path, file: &Path, sha256: &str) -> Result<u8> {
        let mut command = self.command(program);
        command
            .arg("--apply-prepared")
            .arg(file)
            .arg("--prepared-sha256")
            .arg(sha256);
        self.run(command)
    }
    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command.arg("setup").args(&self.arguments);
        command
    }
    fn run(&self, mut command: Command) -> Result<u8> {
        match &self.terminal {
            Some(terminal) => {
                command
                    .stdin(terminal.try_clone()?)
                    .stdout(terminal.try_clone()?)
                    .stderr(terminal.try_clone()?);
            }
            None => {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
            }
        }
        let cancellation = crate::cancellation::Cancellation::new()?;
        cancellation.configure_child(&mut command);
        let output = hive_setup_support::runner::attached_command(
            &mut command,
            self.terminal.as_ref(),
            SETUP_TIMEOUT,
            &|| cancellation.cancelled(),
        )
        .map_err(Into::into);
        let output = cancellation.finish(output)?;
        std::io::stdout().write_all(output.stdout.as_bytes())?;
        std::io::stderr().write_all(output.stderr.as_bytes())?;
        Ok(u8::try_from(output.code).unwrap_or(1))
    }
}
const SETUP_TIMEOUT: Duration = Duration::from_secs(3600);
