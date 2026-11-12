//! The stored marker, and the steps a selection runs before anything changes:
//! server binary, memory floor, verified model, then a ready server for the probe.
use super::{
    Device, Launch, MODEL_CACHE, PREFERRED_PORT, Pin, Progress, READY_DEADLINE, Source, argv,
    download, memory, probe,
};
use crate::{Error, Result, embedding::HttpConfig};
use serde::{Deserialize, Serialize};
use std::{net::TcpListener, path::PathBuf, time::Instant};

const MAX_ARGUMENTS: usize = 64;
const ARGUMENT_BYTES: usize = 4096;

/// Which `llama-server` Comb runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Binary {
    /// `comb-llama-server` next to the running `comb`.
    Bundled,
    /// A user-chosen binary, absolute; it picks its own device.
    Path(PathBuf),
}

/// The marker stored beside a managed HTTP embedding configuration: Comb starts
/// and owns this server whenever it serves the selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedServer {
    pub binary: Binary,
    /// The verified pinned model in Comb's private cache.
    pub model: PathBuf,
    /// Extra arguments for a user-chosen binary, after the validated argv.
    pub arguments: Vec<String>,
}

pub(crate) fn check_arguments(binary: &Binary, arguments: &[String]) -> Result<()> {
    if matches!(binary, Binary::Bundled) && !arguments.is_empty()
        || arguments.len() > MAX_ARGUMENTS
        || arguments
            .iter()
            .any(|a| a.is_empty() || a.len() > ARGUMENT_BYTES || a.contains('\0'))
    {
        return Err(Error::llama_selection_invalid());
    }
    Ok(())
}

impl ManagedServer {
    /// Structural checks of a stored marker and its configuration; returns the port.
    pub(crate) fn validate(&self, http: Option<&HttpConfig>) -> Result<u16> {
        let port = http
            .and_then(argv::port)
            .ok_or_else(Error::llama_selection_invalid)?;
        if !self.model.is_absolute()
            || matches!(&self.binary, Binary::Path(path) if !path.is_absolute())
        {
            return Err(Error::llama_selection_invalid());
        }
        check_arguments(&self.binary, &self.arguments)?;
        Ok(port)
    }

    /// What serve starts: the stored configuration must be exactly the one this
    /// build generates for the binary's device, and the model must still exist.
    pub fn launch(&self, http: &HttpConfig) -> Result<Launch> {
        let port = self.validate(Some(http))?;
        let pin = Pin::shipped()?;
        let (program, device) = probe::resolve(&self.binary)?;
        if *http != argv::config(&pin, device, port) {
            return Err(Error::llama_selection_invalid());
        }
        download::present(&self.model, pin.model.bytes)?;
        Ok(Launch {
            program,
            arguments: argv::argv(&self.model, port, device, &self.arguments),
            port,
            pin,
        })
    }
}

/// A selection whose server binary, memory and model passed their checks.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub server: ManagedServer,
    pub device: Device,
    /// False for a server version this build has not tested: a warning only.
    pub tested: bool,
    pub pin: Pin,
}
impl Prepared {
    /// The managed configuration on `port`.
    pub fn config(&self, port: u16) -> HttpConfig {
        argv::config(&self.pin, self.device, port)
    }
    /// The server for a selection change on `port`.
    pub fn launch(&self, port: u16) -> Result<Launch> {
        self.server.launch(&self.config(port))
    }
}

/// Shared admission before setup activation or a model download.
pub(crate) fn preflight(
    binary: &Binary,
    arguments: &[String],
    pin: &Pin,
    cancelled: &dyn Fn() -> bool,
) -> Result<super::Checked> {
    check_arguments(binary, arguments)?;
    let checked = probe::check_binary(binary, pin, cancelled)?;
    memory::admit(memory::available()?)?;
    Ok(checked)
}

/// Check the server and memory, then download or rehash the pinned model.
pub fn prepare(
    binary: Binary,
    arguments: Vec<String>,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Prepared> {
    let pin = Pin::shipped()?;
    let binary = match binary {
        Binary::Path(path) => Binary::Path(std::path::absolute(path)?),
        Binary::Bundled => Binary::Bundled,
    };
    let checked = preflight(&binary, &arguments, &pin, cancelled)?;
    let url = pin.url();
    let model = download::ensure(
        &cache_directory()?,
        &Source::pinned(&pin, &url),
        cancelled,
        progress,
    )?;
    Ok(Prepared {
        server: ManagedServer {
            binary,
            model,
            arguments,
        },
        device: checked.device,
        tested: checked.tested,
        pin,
    })
}

/// Comb's private model cache: `$XDG_CACHE_HOME/comb/models` or `~/.cache/comb/models`.
/// The selection stores the resolved model path, so a service sees the same file.
pub fn cache_directory() -> Result<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute("XDG_CACHE_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".cache")))
        .map(|base| base.join(MODEL_CACHE))
        .ok_or_else(Error::model_cache_unavailable)
}

/// Keep the previous managed port, so the space identity and vectors survive a
/// reselection; else prefer the documented port; else any free loopback port.
pub fn choose_port(previous: Option<u16>) -> Result<u16> {
    for port in previous.into_iter().chain([PREFERRED_PORT]) {
        if TcpListener::bind((super::HOST, port)).is_ok() {
            return Ok(port);
        }
    }
    Ok(TcpListener::bind((super::HOST, 0))?.local_addr()?.port())
}

/// Run `work` against a ready server, then stop and reap it, whatever `work` did.
/// The probe confirms normalized 4,096-dimensional embeddings first.
pub fn with_server<T>(
    launch: &Launch,
    config: &HttpConfig,
    cancelled: &dyn Fn() -> bool,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let running = launch.spawn(cancelled)?;
    let result = running
        .wait_ready(Instant::now() + READY_DEADLINE, cancelled)
        .and_then(|()| probe::embeddings(config))
        .and_then(|()| work());
    match (result, running.stop()) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => {
            tracing::error!(name: "comb.llama_stop_failed",
                code = secondary.code.as_str(),
                error = &secondary as &(dyn std::error::Error + 'static),
                decision = "preserve_primary_failure");
            Err(primary)
        }
    }
}

/// The managed port of a stored selection, if it is one.
pub(crate) fn previous_port(
    server: Option<&ManagedServer>,
    http: Option<&HttpConfig>,
) -> Option<u16> {
    server.and_then(|server| server.validate(http).ok())
}
