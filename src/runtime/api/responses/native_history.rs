//! Bounded native-cleanup receipts and dry-run file previews.
use crate::config::Provider;
use serde::{Deserialize, Serialize};
pub(crate) const NATIVE_PREVIEW_LIMIT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeHistory {
    Deleted,
    Retained,
    Partial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeWarningReason {
    UnsafePath,
    AmbiguousSession,
    ScanLimit,
    IdentityChanged,
    IncompleteHistory,
    SharedSession,
    InvalidSessionIdentity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeWarning {
    pub agent_id: String,
    #[serde(deserialize_with = "crate::decode::string_enum")]
    pub reason: NativeWarningReason,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeFilePreview {
    pub path: std::path::PathBuf,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeSessionPreview {
    pub agent_id: String,
    #[serde(deserialize_with = "crate::decode::string_enum")]
    pub provider: Provider,
    pub generation: uuid::Uuid,
    pub thread_id: String,
    #[serde(deserialize_with = "super::read::objects")]
    pub files: Vec<NativeFilePreview>,
    pub bytes: u64,
    pub files_truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "Native cleanup fields flatten into purge and GC replies alongside other cleanup \
        counts."
)]
pub(crate) struct NativeCleanup {
    #[serde(deserialize_with = "crate::decode::string_enum")]
    pub native_history: NativeHistory,
    pub native_sessions_deleted: u64,
    pub native_bytes_deleted: u64,
    pub native_sessions_absent: u64,
    pub native_sessions_retained: u64,
    #[serde(deserialize_with = "super::read::objects")]
    pub native_warnings: Vec<NativeWarning>,
    pub native_warning_count: u64,
    #[serde(deserialize_with = "super::read::objects")]
    pub native_preview: Vec<NativeSessionPreview>,
    pub native_preview_truncated: bool,
}
impl NativeCleanup {
    pub fn new(keep: bool) -> Self {
        Self {
            native_history: if keep {
                NativeHistory::Retained
            } else {
                NativeHistory::Deleted
            },
            native_sessions_deleted: 0,
            native_bytes_deleted: 0,
            native_sessions_absent: 0,
            native_sessions_retained: 0,
            native_warnings: Vec::new(),
            native_warning_count: 0,
            native_preview: Vec::new(),
            native_preview_truncated: false,
        }
    }
    pub fn warn(&mut self, agent: &str, reason: NativeWarningReason) {
        if self.native_history != NativeHistory::Retained {
            self.native_history = NativeHistory::Partial;
        }
        self.native_warning_count = self.native_warning_count.saturating_add(1);
        if self.native_warnings.len() < NATIVE_PREVIEW_LIMIT {
            self.native_warnings.push(NativeWarning {
                agent_id: agent.into(),
                reason,
            });
        }
    }
    pub fn add(&mut self, other: Self) {
        self.native_sessions_deleted += other.native_sessions_deleted;
        self.native_bytes_deleted += other.native_bytes_deleted;
        self.native_sessions_absent += other.native_sessions_absent;
        self.native_sessions_retained += other.native_sessions_retained;
        self.native_warning_count += other.native_warning_count;
        if other.native_history == NativeHistory::Partial {
            self.native_history = NativeHistory::Partial;
        }
        self.native_warnings.extend(
            other
                .native_warnings
                .into_iter()
                .take(NATIVE_PREVIEW_LIMIT.saturating_sub(self.native_warnings.len())),
        );
        let room = NATIVE_PREVIEW_LIMIT.saturating_sub(self.native_preview.len());
        self.native_preview_truncated |=
            other.native_preview_truncated || other.native_preview.len() > room;
        self.native_preview
            .extend(other.native_preview.into_iter().take(room));
    }
}
