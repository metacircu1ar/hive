//! Pure CLI adapters for the serving selection.
use super::Result;
use hive_input::{Code, InputError};

const LOCAL_EXAMPLE: &str = "comb model use qwen3-local --llama-server /usr/local/bin/llama-server";
const ARGUMENT_BYTES: usize = 4096;

/// Selection for `model use`: text search, or the managed local Qwen server.
pub fn choice(value: &str) -> Result<String> {
    if value == "none" || value == crate::llama::CHOICE {
        Ok(value.into())
    } else {
        Err(super::invalid(
            "Only `none` (text search) or `qwen3-local` (local embeddings) can be selected \
                by name; use `model use-http` for other embedding servers.",
        ))
    }
}

/// One extra argument for a user-chosen llama-server; it validates the meaning.
pub fn server_argument(value: &str) -> Result<String> {
    if value.is_empty() || value.len() > ARGUMENT_BYTES || value.contains('\0') {
        return Err(super::invalid("Invalid llama-server argument."));
    }
    Ok(value.into())
}

/// `--llama-server` belongs to the local selection only.
pub fn server_requires_local() -> crate::Error {
    match InputError::new(
        Code::ConflictingArguments,
        "Invalid model selection.",
        "--llama-server",
        "--llama-server together with the qwen3-local selection",
        vec![LOCAL_EXAMPLE.into()],
    ) {
        Ok(input) => input.into(),
        Err(cause) => crate::Error::internal_serialization().caused_by(cause),
    }
}

pub fn prefix(value: &str) -> Result<String> {
    if value.len() > crate::embedding::limits::PREFIX_BYTES {
        return Err(crate::Error::invalid_embedding_space());
    }
    Ok(value.into())
}
