//! Dynamic error constructors and native cause retention.
use super::{Error, ErrorCode};

impl Error {
    pub fn io(e: std::io::Error) -> Self {
        Self::new(
            ErrorCode::IoError,
            format!("I/O operation failed ({:?}).", e.kind()),
        )
        .caused_by(e)
    }
    pub fn index(e: rusqlite::Error) -> Self {
        let message = e.sqlite_error().map_or_else(
            || "Index operation failed (database wrapper error).".to_owned(),
            |cause| {
                format!(
                    "Index operation failed (SQLite primary code {}).",
                    cause.extended_code & 255
                )
            },
        );
        Self::new(ErrorCode::IndexError, message).caused_by(e)
    }
    pub fn json(e: serde_json::Error) -> Self {
        Self::new(
            ErrorCode::InvalidJson,
            format!(
                "Invalid JSON ({:?}, line {}, column {}).",
                e.classify(),
                e.line(),
                e.column()
            ),
        )
        .caused_by(e)
    }
    pub fn frontmatter(e: serde_yaml_ng::Error) -> Self {
        let message = e.location().map_or_else(
            || "Invalid YAML.".to_owned(),
            |at| format!("Invalid YAML (line {}, column {}).", at.line(), at.column()),
        );
        Self::new(ErrorCode::InvalidFrontmatter, message).caused_by(e)
    }
}
impl Error {
    pub fn unsupported_index(path: &std::path::Path) -> Self {
        Self::new(
            ErrorCode::UnsupportedIndex,
            format!(
                "Unsupported index structure at {}. Rebuild index.sqlite with sync \
                    --rebuild; an unsupported vectors.sqlite cache may be removed for \
                    recomputation.",
                path.display()
            ),
        )
    }
    pub fn http_input_too_large(limit: usize) -> Self {
        Self::new(
            ErrorCode::EmbeddingInputTooLarge,
            format!("Input exceeds the local HTTP tokenizer limit of {limit} tokens."),
        )
    }
    pub fn http_input_request_byte_limit(limit: usize) -> Self {
        Self::new(
            ErrorCode::EmbeddingInputTooLarge,
            format!("Input exceeds the HTTP request input limit of {limit} UTF-8 bytes."),
        )
    }
    pub fn embedding_input_rejected(status: u16) -> Self {
        Self::new(
            ErrorCode::EmbeddingInputRejected,
            format!("Embedding endpoint rejected input (HTTP {status})."),
        )
    }
    pub fn embedding_http_status(status: u16) -> Self {
        Self::new(
            ErrorCode::EmbeddingTransport,
            format!("Embedding endpoint returned HTTP {status}."),
        )
    }
    pub fn embedding_asset_size(path: &std::path::Path) -> Self {
        Self::new(
            ErrorCode::EmbeddingAssetSize,
            format!("Embedding asset exceeds its size limit: {}", path.display()),
        )
    }
    pub fn http_tokenizer_load(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::new(
            ErrorCode::EmbeddingModel,
            format!("Cannot load HTTP preflight tokenizer: {e}"),
        )
        .caused_by_boxed(e)
    }
    pub fn http_tokenizer_truncation(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::new(
            ErrorCode::EmbeddingModel,
            format!("Cannot disable HTTP tokenizer truncation: {e}"),
        )
        .caused_by_boxed(e)
    }
    pub fn http_tokenizer_encode(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::new(
            ErrorCode::EmbeddingModel,
            format!("Cannot tokenize HTTP input: {e}"),
        )
        .caused_by_boxed(e)
    }
    pub fn llama_memory_insufficient(available: u64, needed: u64) -> Self {
        let basis = if cfg!(target_os = "macos") {
            "physical"
        } else {
            "available"
        };
        Self::new(
            ErrorCode::EmbeddingModel,
            format!(
                "Local embeddings need at least {needed} bytes ({} GiB) of {basis} memory; \
                    measured {available} bytes.",
                needed >> 30
            ),
        )
    }
    pub fn llama_server_exited(code: Option<i32>, signal: Option<i32>) -> Self {
        let status = match (code, signal) {
            (Some(code), _) => format!("exit code {code}"),
            (None, Some(signal)) => format!("signal {signal}"),
            (None, None) => "unknown status".to_owned(),
        };
        Self::new(
            ErrorCode::EmbeddingModel,
            format!("llama-server exited before it became ready ({status})."),
        )
    }
    pub fn llama_server_port_in_use(port: u16) -> Self {
        Self::new(
            ErrorCode::EmbeddingModel,
            format!(
                "Loopback port {port} for the managed llama-server is in use; stop the other \
                    server or rerun comb model use qwen3-local to choose another port."
            ),
        )
    }
    pub fn model_download_status(status: u16) -> Self {
        Self::new(
            ErrorCode::EmbeddingTransport,
            format!("Model download returned HTTP {status}; rerun the command to resume it."),
        )
    }
    pub fn model_disk_space(needed: u64) -> Self {
        Self::new(
            ErrorCode::SizeLimit,
            format!("The model download needs {needed} more bytes of free disk space."),
        )
    }
    pub fn busy(lane: &str) -> Self {
        Self::new(
            ErrorCode::CombBusy,
            format!("Comb {lane} capacity is unavailable."),
        )
    }
}

impl Error {
    pub fn credential_save_failed(id: impl std::fmt::Display, error: Self, revoked: bool) -> Self {
        let committed = error.file_committed || !revoked;
        let message = if revoked {
            format!("Credential {id} could not be saved and has been revoked.")
        } else {
            format!(
                "Credential {id} could not be saved; revocation is unconfirmed. Inspect \
                    credential list and revoke it with credential revoke {id}."
            )
        };
        let mut mapped = Self::new(error.code, message).caused_by(error);
        mapped.file_committed = committed;
        mapped
    }
    pub fn credential_publication_uncertain(id: impl std::fmt::Display, error: Self) -> Self {
        Self::new(
            error.code,
            format!(
                "Credential {id} was published, but its durability or output path is \
            uncertain. It remains active; inspect the file before retrying."
            ),
        )
        .committed()
        .caused_by(error)
    }
    pub fn individual_input_rejected(error: Self) -> Self {
        Self::new(
            error.code,
            if error.code == ErrorCode::EmbeddingInputTooLarge {
                "Composed input exceeds its byte or tokenizer limit."
            } else {
                "Embedding endpoint rejected this individual input; inspect content and \
                model configuration, then retry failed inputs."
            },
        )
        .caused_by(error)
    }
}

impl Error {
    pub fn io_open(e: std::io::Error) -> Self {
        Self::new(
            ErrorCode::IoError,
            format!("I/O operation failed ({:?}).", e.kind()),
        )
        .caused_by(super::OpeningFile(e))
    }
}

impl Error {
    pub fn setup_configuration(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidEmbeddingConfig, message)
    }
    pub fn provision_configuration(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidEmbeddingConfig, message)
    }
    pub fn maintenance_configuration(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidArguments, message)
    }
    pub fn service_io(error: &std::io::Error) -> Self {
        Self::new(ErrorCode::IoError, error.to_string())
    }
    pub fn service_restart_failed(mut error: Self, restart: &std::io::Error) -> Self {
        error.message = format!("{}; service restart failed: {}", error.message, restart).into();
        error
    }
    pub fn service_environment_missing(missing: &[String]) -> Self {
        Self::new(
            ErrorCode::InvalidEmbeddingConfig,
            format!("Service environment is missing: {}", missing.join(", ")),
        )
    }
    pub fn service_environment_required(missing: &[String]) -> Self {
        Self::new(
            ErrorCode::InvalidArguments,
            format!(
                "Service needs --service-environment containing: {}",
                missing.join(", ")
            ),
        )
    }
    pub fn service_model_environment_missing(missing: &[String]) -> Self {
        Self::new(
            ErrorCode::InvalidArguments,
            format!(
                "Owned service environment file needs: {}. No model change was made.",
                missing.join(", ")
            ),
        )
    }
}
