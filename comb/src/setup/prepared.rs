//! Retain explicit caller configuration across state reinspection and service pause.
use super::{Error, Options, Path, Result, input};
use crate::embedding::{Config, HttpConfig};
use hive_input::Code;

#[derive(Clone)]
pub struct Prepared {
    pub(super) options: Options,
    pub(super) http: Option<HttpConfig>,
    /// A resolved local selection: its server marker, with `http` its configuration.
    pub(super) llama_server: Option<crate::llama::ManagedServer>,
    /// The caller's download progress display, including the model's license line.
    progress: Option<crate::llama::Reporter>,
}
impl Options {
    pub fn prepare(self) -> Result<Prepared> {
        self.validate_input()?;
        let config = self
            .embedding_config
            .as_deref()
            .map(read_config)
            .transpose()?;
        Prepared::from_config(self, config)
    }
}
impl Prepared {
    /// Validate the actual staged binary before an installer activates it.
    /// Keep `Bundled` in the saved answers; staging paths are never selections.
    pub(crate) fn check_local(&self) -> Result<()> {
        let Some((mut binary, arguments)) = self.local_request() else {
            return Ok(());
        };
        if matches!(binary, crate::llama::Binary::Bundled)
            && let Some(path) = &self.options.preparation.prepare_resources
        {
            let resources: hive_setup_support::preparation::Resources =
                hive_setup_support::preparation::read(path, None)?;
            resources.validate()?;
            let source = resources
                .llama_source
                .ok_or_else(Error::llama_server_missing)?;
            binary = crate::llama::Binary::Path(source.join("bin").join(crate::llama::BUNDLED));
        }
        let signals = hive_setup_support::runner::Signals::new()?;
        let result =
            crate::llama::preflight(&binary, &arguments, &crate::llama::Pin::shipped()?, &|| {
                signals.cancelled()
            });
        signals.finish(result).map(|_| ())
    }

    /// Use a configuration already read by a CLI input adapter, including pipe input.
    pub fn from_config(options: Options, config: Option<Config>) -> Result<Self> {
        options.validate_input()?;
        if options.embedding_config.is_some() != config.is_some() {
            return Err(Error::internal_serialization());
        }
        let http = config.map(http).transpose()?;
        Ok(Self {
            options,
            http,
            llama_server: None,
            progress: None,
        })
    }
    pub fn options(&self) -> &Options {
        &self.options
    }
    /// The requested local selection whose binary, memory and model checks are
    /// still to run: the binary and its extra arguments.
    pub fn local_request(&self) -> Option<(crate::llama::Binary, Vec<String>)> {
        (self.options.local_embeddings && self.llama_server.is_none()).then(|| {
            (
                self.options
                    .llama_server
                    .clone()
                    .map_or(crate::llama::Binary::Bundled, crate::llama::Binary::Path),
                self.options.llama_server_arg.clone(),
            )
        })
    }
    /// Choose local embeddings, as an interactive answer does.
    #[must_use]
    pub fn request_local(mut self) -> Self {
        self.options.local_embeddings = true;
        self
    }
    /// Show download progress, and the model's license line, through `report`.
    #[must_use]
    pub fn with_progress(mut self, report: crate::llama::Reporter) -> Self {
        self.progress = Some(report);
        self
    }
    pub fn progress(&self) -> Option<&crate::llama::Reporter> {
        self.progress.as_ref()
    }
    /// The resolved local selection: its HTTP configuration and server marker.
    #[must_use]
    pub fn with_local(mut self, http: HttpConfig, server: crate::llama::ManagedServer) -> Self {
        self.http = Some(http);
        self.llama_server = Some(server);
        self
    }
}
fn http(config: Config) -> Result<HttpConfig> {
    let Config::Http(config) = config;
    Config::Http(config.clone())
        .validate_input()
        .map_err(|cause| {
            let expected = cause
                .input()
                .map_or("valid HTTP embedding configuration", |input| {
                    input.expected()
                });
            input::error(Code::InvalidValue, "embedding_config", expected).caused_by(cause)
        })?;
    Ok(config)
}
fn read_config(path: &Path) -> Result<Config> {
    Config::read(path).map_err(|cause| {
        if cause.code == crate::ErrorCode::InvalidEmbeddingConfig || caller_io(&cause) {
            input::error(
                Code::InvalidFile,
                "embedding_config",
                "a readable regular file containing valid HTTP embedding configuration",
            )
            .caused_by(cause)
        } else {
            cause
        }
    })
}
fn caller_io(error: &Error) -> bool {
    use std::error::Error as _;
    let Some(cause) = error
        .source()
        .and_then(|cause| cause.downcast_ref::<std::io::Error>())
    else {
        return false;
    };
    matches!(
        cause.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::IsADirectory
    ) || matches!(
        cause.raw_os_error(),
        Some(libc::ELOOP | libc::ENAMETOOLONG | libc::ENXIO | libc::ENODEV | libc::EOPNOTSUPP)
    )
}
