//! Wire names are defined once; unknown codes are rejected during deserialization.
use serde::{Deserialize, Serialize};
macro_rules! codes {
    ($($(#[$attr:meta])* $variant:ident => ($wire:literal, $report:ident)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ErrorCode { $($(#[$attr])* $variant),+, Input(hive_input::Code) }
        impl ErrorCode {
            // Classification is required alongside every wire name, so a new
            // code cannot accidentally inherit a default diagnostic severity.
            pub(super) const fn is_request_outcome(self) -> bool {
                match self {
                    $($(#[$attr])* Self::$variant => codes!(@report $report),)+
                    Self::Input(_) => true,
                }
            }
            pub const fn as_str(self) -> &'static str {
                match self { $($(#[$attr])* Self::$variant => $wire),
                    +,
                     Self::Input(code) => code.as_str() }
            }
        }
        impl<'de> Deserialize<'de> for ErrorCode {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let name = String::deserialize(d)?;
                match name.as_str() {
                    $($(#[$attr])* $wire => Ok(Self::$variant),)+
                    value => hive_input::Code::deserialize(
                        serde::de::value::StrDeserializer::<D::Error>::new(value)
                    ).map(Self::Input),
                }
            }
        }
    };
    (@report Reply) => { true };
    (@report Operator) => { false };
}
impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl From<hive_input::Code> for ErrorCode {
    fn from(code: hive_input::Code) -> Self {
        match code {
            hive_input::Code::InvalidJson => Self::InvalidJson,
            hive_input::Code::InvalidRequestId => Self::InvalidRequestId,
            code => Self::Input(code),
        }
    }
}
codes! {
    IncompatibleProtocol => ("incompatible_protocol", Operator),
    UnknownError => ("unknown_error", Operator),
    CombBusy => ("comb_busy", Reply),
    CombUnavailable => ("comb_unavailable", Operator),
    ChunkLimit => ("chunk_limit", Reply),
    Conflict => ("conflict", Reply),
    ConnectionLost => ("connection_lost", Operator),
    ConsolidationConflict => ("consolidation_conflict", Reply),
    ConsolidationLimit => ("consolidation_limit", Reply),
    CredentialExists => ("credential_exists", Reply),
    CredentialLimit => ("credential_limit", Reply),
    InvalidEmbeddingAsset => ("invalid_embedding_asset", Operator),
    EmbeddingAssetSize => ("embedding_asset_size", Operator),
    EmbeddingCredentials => ("embedding_credentials", Operator),
    EmbeddingEndpointRejected => ("embedding_endpoint_rejected", Operator),
    EmbeddingInputRejected => ("embedding_input_rejected", Reply),
    EmbeddingInputTooLarge => ("embedding_input_too_large", Reply),
    EmbeddingModel => ("embedding_model", Operator),
    EmbeddingReplaceRequired => ("embedding_replace_required", Operator),
    EmbeddingSpaceChanged => ("embedding_space_changed", Reply),
    EmbeddingTransport => ("embedding_transport", Operator),
    EmbeddingsBusy => ("embeddings_busy", Reply),
    EmbeddingsUnavailable => ("embeddings_unavailable", Operator),
    #[cfg(any(test, feature = "test-support"))]
    FixtureUnavailable => ("fixture_unavailable", Operator),
    Forbidden => ("forbidden", Reply),
    GraphLimit => ("graph_limit", Reply),
    InvalidGraphQuery => ("invalid_graph_query", Reply),
    GraphIncomplete => ("graph_incomplete", Reply),
    ImmutableTimeline => ("immutable_timeline", Reply),
    ImportCollision => ("import_collision", Reply),
    ImportLimit => ("import_limit", Reply),
    IndexError => ("index_error", Operator),
    InternalError => ("internal_error", Operator),
    InvalidActor => ("invalid_actor", Operator),
    InvalidArguments => ("invalid_arguments", Reply),
    InvalidCacheBudget => ("invalid_cache_budget", Reply),
    InvalidCapture => ("invalid_capture", Reply),
    InvalidConsolidation => ("invalid_consolidation", Reply),
    InvalidCredential => ("invalid_credential", Operator),
    InvalidDecision => ("invalid_decision", Reply),
    InvalidDepth => ("invalid_depth", Reply),
    InvalidEmbedding => ("invalid_embedding", Operator),
    InvalidEmbeddingBatch => ("invalid_embedding_batch", Operator),
    InvalidEmbeddingConfig => ("invalid_embedding_config", Operator),
    InvalidEmbeddingState => ("invalid_embedding_state", Operator),
    InvalidEvidence => ("invalid_evidence", Reply),
    InvalidFrame => ("invalid_frame", Reply),
    InvalidFrontmatter => ("invalid_frontmatter", Operator),
    InvalidIdentity => ("invalid_identity", Operator),
    InvalidImport => ("invalid_import", Reply),
    InvalidJson => ("invalid_json", Operator),
    InvalidLimit => ("invalid_limit", Reply),
    InvalidOperation => ("invalid_operation", Reply),
    InvalidPage => ("invalid_page", Operator),
    InvalidRequest => ("invalid_request", Reply),
    InvalidRequestId => ("invalid_request_id", Reply),
    InvalidSearch => ("invalid_search", Reply),
    InvalidSlug => ("invalid_slug", Operator),
    InvalidSocket => ("invalid_socket", Reply),
    InvalidState => ("invalid_state", Operator),
    InvalidTimeline => ("invalid_timeline", Operator),
    IoError => ("io_error", Operator),
    NotFound => ("not_found", Reply),
    PageLimit => ("page_limit", Reply),
    RecoveryRequired => ("recovery_required", Operator),
    RequestConflict => ("request_conflict", Reply),
    ResultRemoved => ("result_removed", Reply),
    ServerEmbeddingConfig => ("server_embedding_config", Operator),
    SizeLimit => ("size_limit", Operator),
    SocketPathTooLong => ("socket_path_too_long", Reply),
    #[cfg(any(test, feature = "test-support"))]
    TestInterrupted => ("test_interrupted", Operator),
    Timeout => ("timeout", Operator),
    Uncertain => ("uncertain", Operator),
    UnknownTool => ("unknown_tool", Reply),
    UnsafeCredential => ("unsafe_credential", Operator),
    UnsafePath => ("unsafe_path", Operator),
    UnsafeSocket => ("unsafe_socket", Operator),
    UnsupportedIndex => ("unsupported_index", Operator),
    UnsupportedFormat => ("unsupported_format", Operator),
    NewerFormat => ("newer_format", Operator),
    WorkerFailed => ("worker_failed", Operator),
}
impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
