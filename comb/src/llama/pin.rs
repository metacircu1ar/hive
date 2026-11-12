//! The one shared pin: llama.cpp commit and tree, and the Qwen revision, SHA-256
//! and size. The release builder and the opt-in end-to-end suite read the same
//! file; a placeholder or malformed value refuses local embeddings here too.
use crate::{Error, Result};
use serde::Deserialize;
use std::path::Path;

const SHIPPED: &str = include_str!("pin.json");
const REPOSITORY: &str = "https://github.com/ggml-org/llama.cpp.git";
const LICENSE: &str = "Apache-2.0";
/// The pinned file's quantization; it prefixes the revision in the space identity.
const QUANTIZATION: &str = "q8_0";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    llama_cpp: Source,
    model: Model,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    repository: String,
    commit: String,
    tree: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub repository: String,
    pub revision: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    pub license: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    /// The llama.cpp commit the bundled server is built from.
    pub commit: String,
    pub model: Model,
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

impl Pin {
    /// The pin compiled into this build.
    pub fn shipped() -> Result<Self> {
        Self::parse(SHIPPED)
    }
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let raw: Raw = hive_input::decode::from_str(text)
            .map_err(|cause| Error::llama_pin_unverified().caused_by(cause))?;
        let model = &raw.model;
        let repository = model
            .repository
            .split_once('/')
            .is_some_and(|(owner, project)| name(owner) && name(project));
        if raw.llama_cpp.repository != REPOSITORY
            || !hex(&raw.llama_cpp.commit, 40)
            || !hex(&raw.llama_cpp.tree, 40)
            || !repository
            || !hex(&model.revision, 40)
            || !hex(&model.sha256, 64)
            || model.bytes == 0
            || !name(&model.file)
            || Path::new(&model.file).extension() != Some(std::ffi::OsStr::new("gguf"))
            || model.license != LICENSE
        {
            return Err(Error::llama_pin_unverified());
        }
        Ok(Self {
            commit: raw.llama_cpp.commit,
            model: raw.model,
        })
    }
    /// The pinned file at its pinned revision; the content is verified separately.
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.model.repository, self.model.revision, self.model.file
        )
    }
    /// The model revision in the embedding space identity: quantization and file hash.
    pub fn revision(&self) -> String {
        format!("{QUANTIZATION}-{}", self.model.sha256)
    }
    /// A reported build commit names a tested server when it prefixes the pin.
    pub fn tested(&self, commit: &str) -> bool {
        commit.len() >= 7 && hex(commit, commit.len()) && self.commit.starts_with(commit)
    }
}
