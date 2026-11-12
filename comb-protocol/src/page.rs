//! Wire attribution and review decisions.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Actor {
    Human,
    Client {
        id: String,
        name: String,
    },
    External,
    Agent {
        bridge: String,
        id: String,
        role: Option<String>,
        execution: String,
        turn: String,
    },
    Collector {
        bridge: String,
    },
}
impl Actor {
    pub fn is_human(&self) -> bool {
        matches!(self, Self::Human)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accept,
    Reject,
}
pub fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
