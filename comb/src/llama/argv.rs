//! The validated server argv and the HTTP embedding configuration it serves.
use super::{
    ALIAS, CPU_SLOT_TOKENS, CPU_TIMEOUT_SECONDS, DIMENSIONS, HOST, Pin, QUERY_PREFIX, SLOT_TOKENS,
    SLOTS, TIMEOUT_SECONDS,
};
use crate::embedding::HttpConfig;
use std::{ffi::OsString, path::Path};

const ENDPOINT: &str = "/v1/embeddings";

/// Where the server computes. The bundled macOS build uses Metal and the bundled
/// Linux build uses the CPU; a user-chosen binary picks its own device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Metal,
    Cpu,
    Own,
}
impl Device {
    /// The device of this platform's bundled `comb-llama-server`.
    pub const fn bundled() -> Self {
        if cfg!(target_os = "macos") {
            Self::Metal
        } else {
            Self::Cpu
        }
    }
    /// Tokens per slot, which bounds one request input.
    pub const fn slot_tokens(self) -> usize {
        match self {
            Self::Cpu => CPU_SLOT_TOKENS,
            Self::Metal | Self::Own => SLOT_TOKENS,
        }
    }
    pub const fn timeout_seconds(self) -> u64 {
        match self {
            Self::Cpu => CPU_TIMEOUT_SECONDS,
            Self::Metal | Self::Own => TIMEOUT_SECONDS,
        }
    }
    const fn flags(self) -> &'static [&'static str] {
        match self {
            Self::Metal => &["--device", "MTL0", "--n-gpu-layers", "all"],
            Self::Cpu => &["--device", "none", "--n-gpu-layers", "0"],
            Self::Own => &[],
        }
    }
}

/// Last-token pooling, normalized 4,096-dimensional vectors and four slots. Flash
/// attention is pinned: without it a full slot needs far more working memory than
/// the memory floor allows. User arguments follow, so they can override the rest.
pub fn argv(model: &Path, port: u16, device: Device, extra: &[String]) -> Vec<OsString> {
    let context = (SLOTS * device.slot_tokens()).to_string();
    let slot = device.slot_tokens().to_string();
    let slots = SLOTS.to_string();
    let port = port.to_string();
    let mut argv = vec![OsString::from("--model"), model.as_os_str().to_owned()];
    argv.extend(
        [
            "--alias",
            ALIAS,
            "--embedding",
            "--pooling",
            "last",
            "--embd-normalize",
            "2",
        ]
        .map(OsString::from),
    );
    argv.extend(device.flags().iter().map(OsString::from));
    argv.extend(
        [
            "--flash-attn",
            "on",
            "--ctx-size",
            context.as_str(),
            "--parallel",
            slots.as_str(),
            "--no-kv-unified",
            "--batch-size",
            slot.as_str(),
            "--ubatch-size",
            slot.as_str(),
            "--cache-ram",
            "0",
            "--host",
            HOST,
            "--port",
            port.as_str(),
            "--no-webui",
            "--no-context-shift",
        ]
        .map(OsString::from),
    );
    argv.extend(extra.iter().map(OsString::from));
    argv
}

/// The embedding configuration a managed server on `port` serves.
pub fn config(pin: &Pin, device: Device, port: u16) -> HttpConfig {
    HttpConfig {
        model: ALIAS.into(),
        revision: pin.revision(),
        url: format!("http://{HOST}:{port}{ENDPOINT}"),
        dimensions: DIMENSIONS,
        document_prefix: String::new(),
        query_prefix: QUERY_PREFIX.into(),
        api_key_env: None,
        timeout_seconds: device.timeout_seconds(),
        request_dimensions: false,
        tokenizer: None,
        max_tokens: None,
        // Qwen's byte-level tokenizer cannot exceed the input byte count plus
        // its special tokens. Bound the whole CPU request, not just each slot:
        // several individually valid pieces can otherwise exceed its deadline.
        max_request_bytes: (device == Device::Cpu).then_some(super::CPU_REQUEST_BYTES),
    }
}

/// The port of a managed configuration's exact loopback URL.
pub(crate) fn port(config: &HttpConfig) -> Option<u16> {
    let port = config
        .url
        .strip_prefix(&format!("http://{HOST}:"))?
        .strip_suffix(ENDPOINT)?;
    let parsed = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    (parsed.to_string() == port).then_some(parsed)
}
