use super::{Context, Exit};
use clap::{Args, Subcommand};
use comb::{Result, embedding::HttpConfig, error::value, llama, provision};
use hive_input::clap::parsers;
use std::{io::IsTerminal as _, path::PathBuf};

mod input;

#[derive(Subcommand)]
pub(super) enum ModelCommand {
    /// Serve text search only ("none"), or local embeddings ("qwen3-local"): download
    /// the pinned 8 GB Qwen model and run the bundled llama-server for --root.
    Use {
        #[arg(value_parser = provision::input::choice)]
        name: String,
        /// Run this llama-server binary instead of the bundled comb-llama-server.
        #[arg(long)]
        llama_server: Option<PathBuf>,
        /// Extra argument for the --llama-server binary, after Comb's own (repeat).
        #[arg(
            long,
            requires = "llama_server",
            allow_hyphen_values = true,
            value_parser = provision::input::server_argument
        )]
        llama_server_arg: Vec<String>,
    },
    /// Validate an external embedding endpoint, then select it for --root.
    UseHttp(Http),
}
#[derive(Args)]
pub(super) struct Http {
    #[arg(long, value_parser = input::url)]
    url: String,
    #[arg(long, value_parser = input::identity)]
    model: String,
    #[arg(long, value_parser = parsers::count(HttpConfig::DIMENSIONS))]
    dimensions: usize,
    /// Pin a server-managed model revision; change it when its weights change.
    #[arg(long, default_value = "operator-managed", value_parser = input::identity)]
    revision: String,
    #[arg(long, default_value = "", value_parser = input::prefix)]
    query_prefix: String,
    #[arg(long, default_value = "", value_parser = input::prefix)]
    document_prefix: String,
    #[arg(long)]
    api_key_env: Option<String>,
    #[arg(long, default_value_t = 60,
         value_parser = parsers::unsigned(HttpConfig::TIMEOUT_SECONDS))]
    timeout_seconds: u64,
    #[arg(long)]
    request_dimensions: bool,
    #[arg(long, requires = "max_tokens")]
    tokenizer: Option<PathBuf>,
    #[arg(long, requires = "tokenizer", value_parser = parsers::count(HttpConfig::MAX_TOKENS))]
    max_tokens: Option<usize>,
    /// Bound total UTF-8 input bytes per request, including prefixes.
    #[arg(long, value_parser = parsers::count(HttpConfig::REQUEST_BYTES))]
    max_request_bytes: Option<usize>,
}
impl From<Http> for HttpConfig {
    fn from(h: Http) -> Self {
        Self {
            url: h.url,
            model: h.model,
            dimensions: h.dimensions,
            revision: h.revision,
            query_prefix: h.query_prefix,
            document_prefix: h.document_prefix,
            api_key_env: h.api_key_env,
            timeout_seconds: h.timeout_seconds,
            request_dimensions: h.request_dimensions,
            tokenizer: h.tokenizer,
            max_tokens: h.max_tokens,
            max_request_bytes: h.max_request_bytes,
        }
    }
}
pub(super) fn execute(context: &Context, command: ModelCommand) -> Result<Exit> {
    let json = match command {
        ModelCommand::Use {
            name,
            llama_server,
            llama_server_arg,
        } if name == llama::CHOICE => use_local(context, llama_server, llama_server_arg)?,
        ModelCommand::Use { llama_server, .. } if llama_server.is_some() => {
            return Err(provision::input::server_requires_local());
        }
        ModelCommand::Use { .. } => value(comb::services::paused(&context.comb, || {
            provision::select_none(&context.comb)
        })?)?,
        ModelCommand::UseHttp(args) => {
            let config: HttpConfig = args.into();
            comb::services::validate_environment(
                &context.comb,
                &config.api_key_env.clone().into_iter().collect::<Vec<_>>(),
            )?;
            // Validate URL, tokenizer and credentials before stopping the old service.
            provision::validate_http(config.clone())?;
            value(comb::services::paused(&context.comb, || {
                provision::use_http(&context.comb, config)
            })?)?
        }
    };
    Ok(Exit::Json(json))
}

/// Server binary, memory floor and verified download run while any old service
/// still serves. The selection change then runs paused, with the new server ready.
fn use_local(
    context: &Context,
    path: Option<PathBuf>,
    arguments: Vec<String>,
) -> Result<serde_json::Value> {
    // Inspect the root's settings first: a missing root fails before any download.
    let previous = provision::settings(&context.comb)?.and_then(|saved| saved.llama_port());
    let binary = path.map_or(llama::Binary::Bundled, llama::Binary::Path);
    let prepared = {
        let signals = hive_setup_support::runner::Signals::new()?;
        let report = reporter()?;
        let result = llama::prepare(binary, arguments, &|| signals.cancelled(), &mut |event| {
            report(event);
        });
        signals.finish(result)?
    };
    let selected = comb::services::paused(&context.comb, || {
        let port = llama::choose_port(previous)?;
        let config = prepared.config(port);
        let launch = prepared.launch(port)?;
        let signals = hive_setup_support::runner::Signals::new()?;
        let result = llama::with_server(&launch, &config, &|| signals.cancelled(), || {
            provision::use_llama(&context.comb, config.clone(), prepared.server.clone())
        });
        signals.finish(result)
    })?;
    let mut json = value(selected)?;
    if let Some(object) = json.as_object_mut() {
        let warnings: &[&str] = if prepared.tested {
            &[]
        } else {
            &["untested_llama_server_version"]
        };
        object.insert("warnings".into(), serde_json::json!(warnings));
    }
    Ok(json)
}

/// Human download progress on stderr, shared by `model use` and `setup`.
pub(super) fn reporter() -> Result<llama::Reporter> {
    let report = std::sync::Mutex::new(Report::new(llama::Pin::shipped()?));
    Ok(std::sync::Arc::new(move |event: llama::Progress| {
        report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .show(event);
    }))
}

/// Human download progress on stderr; JSON results stay on stdout.
struct Report {
    pin: llama::Pin,
    terminal: bool,
    shown: Option<u64>,
}
impl Report {
    fn new(pin: llama::Pin) -> Self {
        Self {
            pin,
            terminal: std::io::stderr().is_terminal(),
            shown: None,
        }
    }
    fn show(&mut self, event: llama::Progress) {
        match event {
            llama::Progress::Started { offset, total } => {
                let action = if offset == 0 {
                    "Downloading"
                } else {
                    "Resuming"
                };
                eprintln!(
                    "{action} {} ({total} bytes) from {} into Comb's model cache. \
                        The model is licensed under {}.",
                    self.pin.model.file, self.pin.model.repository, self.pin.model.license
                );
            }
            llama::Progress::Received { offset, total } if self.terminal && total > 0 => {
                let percent = offset.saturating_mul(100) / total;
                if self.shown.is_none_or(|shown| percent >= shown + 5) {
                    eprintln!("Downloaded {percent}%");
                    self.shown = Some(percent);
                }
            }
            llama::Progress::Received { .. } => (),
            llama::Progress::Verifying => eprintln!("Verifying the model's SHA-256."),
        }
    }
}
