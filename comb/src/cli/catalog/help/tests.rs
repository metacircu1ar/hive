//! Frozen help mapping checks the data table against its original behavior.
use super::{description, graph, model};

// Recorded from the pre-S7d source. Do not refresh this oracle to hide a regression.
fn before(path: &str, id: &str) -> Option<&'static str> {
    if path == "graph-query"
        && let Some(help) = graph(id)
    {
        return Some(help);
    }
    Some(match (path, id) {
        (_, "comb") => {
            "Knowledge root (except version/mcp; current directory, setup/service: $HOME/Comb)"
        }
        (_, "socket") => {
            "Service socket for setup, serve, mcp and remote commands (not sync --rebuild)"
        }
        (_, "run") => "Run name for put, append, capture, promote, consolidate and decide",
        ("version", "json") => "Print the installed binary identity as JSON",
        ("mcp", "credential") => {
            "Private client connection file containing the socket and credential"
        }
        ("init", "dir") => "Knowledge directory to initialize (defaults to --root)",
        ("import", "dir") => "Directory of Markdown files to import or resume",
        ("import", "status") => {
            "Show the retained import batch and confirmed progress without reading its source"
        }
        ("import", "abandon") => {
            "Abandon this exact import batch ID, preserving completed pages and receipts"
        }
        (_, "slug") => "Knowledge page slug, for example components/writer",
        ("put", "file") => "UTF-8 Markdown file to read (stdin when omitted)",
        (_, "expected_hash") => "Current page hash required for the intended edit",
        (_, "request_id") => {
            "Stable caller request ID; reuse it with identical input when retrying"
        }
        ("append", "text") => "Evidence text to append",
        ("capture", "text") => "Inbox note text",
        ("append", "source") => "Evidence source assertion, for example commit:abc123",
        (_, "summary_file") => "UTF-8 replacement summary (append also requires --expected-hash)",
        ("consolidate", "note") => "Inbox note slug to consolidate",
        ("consolidate", "target") => "Existing ordinary page receiving the note's evidence",
        ("consolidate", "note_hash") => "Current hash of the inbox note",
        ("consolidate", "target_hash") => "Current hash of the target page",
        ("decide", "decision") => "Whether to accept or reject the proposal",
        ("decide", "reason") => "Explanation recorded with the review decision",
        (_, "kind") => "Filter by page metadata type",
        (_, "tag") => "Filter by one page tag",
        ("import", "prefix") => "Slug prefix for imported pages, for example operations",
        (_, "prefix") => "Filter by literal slug prefix, for example components/",
        (_, "unverified") => "Return only pages with unverified content",
        ("search" | "graph-query", "after") => {
            "Include pages updated at or after this RFC 3339 timestamp"
        }
        ("search" | "graph-query", "before") => {
            "Include pages updated at or before this RFC 3339 timestamp"
        }
        (_, "after") => "Continue after this page slug",
        ("search", "limit") => "Maximum result pages (1-100)",
        (_, "limit") => "Maximum rows in this page (1-1000)",
        (_, "query") => "Search query text",
        (_, "mode") => "Literal text, semantic vector, or combined hybrid retrieval",
        (_, "embedding_config") => "Embedding configuration file",
        (_, "depth") => "Maximum graph distance from the starting page (0-8)",
        (_, "config") => "Embedding configuration file for offline indexing",
        (_, "replace") => "Replace the currently active embedding space",
        (_, "replace_embeddings") => "Allow this server to replace the active embedding space",
        (_, "retry_failed") => "Retry chunks previously rejected during indexing",
        (_, "vector_cache_mib") => {
            "Maximum in-memory vector cache size in MiB (1-8192; default 512 with a model)"
        }
        (_, "rebuild") => "Rebuild the derived text index while retaining durable knowledge",
        ("credential create", "name") => "Human-readable label for this client credential",
        ("credential create", "out") => "New private connection file to write",
        ("credential create", "bridge") => "Issue a Hive bridge credential",
        ("credential create", "reviewer") => "Allow this client to review knowledge",
        (_, "reviewer_role") => {
            "Hive role allowed to review through the bridge (repeat for multiple roles)"
        }
        ("credential revoke", "id") => "UUID of the credential to revoke",
        (_, "bridge_file") => "Private Hive bridge connection file to create or reuse",
        _ => return model(path, id),
    })
}

const REPRESENTATIVES: &[&str] = &[
    "comb",
    "socket",
    "credential",
    "file",
    "summary_file",
    "config",
    "embedding_config",
    "out",
    "tokenizer",
    "dir",
    "ROOT",
    "kind",
    "TYPE",
    "graph-query",
    "Knowledge root (except version/mcp; current directory, setup/service: $HOME/Comb)",
    "Service socket for setup, serve, mcp and remote commands (not sync --rebuild)",
    "run",
    "Run name for put, append, capture, promote, consolidate and decide",
    "version",
    "json",
    "Print the installed binary identity as JSON",
    "mcp",
    "Private client connection file containing the socket and credential",
    "init",
    "Knowledge directory to initialize (defaults to --root)",
    "import",
    "Directory of Markdown files to import or resume",
    "status",
    "Show the retained import batch and confirmed progress without reading its source",
    "abandon",
    "Abandon this exact import batch ID, preserving completed pages and receipts",
    "slug",
    "Knowledge page slug, for example components/writer",
    "put",
    "UTF-8 Markdown file to read (stdin when omitted)",
    "expected_hash",
    "Current page hash required for the intended edit",
    "request_id",
    "Stable caller request ID; reuse it with identical input when retrying",
    "append",
    "text",
    "Evidence text to append",
    "capture",
    "Inbox note text",
    "source",
    "Evidence source assertion, for example commit:abc123",
    "UTF-8 replacement summary (append also requires --expected-hash)",
    "consolidate",
    "note",
    "Inbox note slug to consolidate",
    "target",
    "Existing ordinary page receiving the note's evidence",
    "note_hash",
    "Current hash of the inbox note",
    "target_hash",
    "Current hash of the target page",
    "decide",
    "decision",
    "Whether to accept or reject the proposal",
    "reason",
    "Explanation recorded with the review decision",
    "Filter by page metadata type",
    "tag",
    "Filter by one page tag",
    "prefix",
    "Slug prefix for imported pages, for example operations",
    "Filter by literal slug prefix, for example components/",
    "unverified",
    "Return only pages with unverified content",
    "search",
    "after",
    "Include pages updated at or after this RFC 3339 timestamp",
    "before",
    "Include pages updated at or before this RFC 3339 timestamp",
    "Continue after this page slug",
    "limit",
    "Maximum result pages (1-100)",
    "Maximum rows in this page (1-1000)",
    "query",
    "Search query text",
    "mode",
    "Literal text, semantic vector, or combined hybrid retrieval",
    "Embedding configuration file",
    "depth",
    "Maximum graph distance from the starting page (0-8)",
    "Embedding configuration file for offline indexing",
    "replace",
    "Replace the currently active embedding space",
    "replace_embeddings",
    "Allow this server to replace the active embedding space",
    "retry_failed",
    "Retry chunks previously rejected during indexing",
    "vector_cache_mib",
    "Maximum in-memory vector cache size in MiB (1-8192; default 512 with a model)",
    "rebuild",
    "Rebuild the derived text index while retaining durable knowledge",
    "credential create",
    "name",
    "Human-readable label for this client credential",
    "New private connection file to write",
    "bridge",
    "Issue a Hive bridge credential",
    "reviewer",
    "Allow this client to review knowledge",
    "reviewer_role",
    "Hive role allowed to review through the bridge (repeat for multiple roles)",
    "credential revoke",
    "id",
    "UUID of the credential to revoke",
    "bridge_file",
    "Private Hive bridge connection file to create or reuse",
    "to",
    "Destination page slug for one shortest connection",
    "direction",
    "Follow outgoing, incoming or both directions",
    "Maximum relation distance from the starting page (0-4)",
    "Maximum returned relations (1-200)",
    "max_nodes",
    "Maximum nodes visited across the query (1-1000)",
    "max_edges",
    "Maximum edges inspected across the query (1-5000)",
    "max_evidence_bytes",
    "Maximum evidence bytes returned across the query (1-262144)",
    "deadline_ms",
    "Query deadline in milliseconds (1-500)",
    "model use-http",
    "dimensions",
    "Embedding vector dimension count (1-4096)",
    "max_tokens",
    "Maximum tokenizer input length (16-8192; requires --tokenizer)",
    "query_prefix",
    "document_prefix",
    "Text prepended to embeddings (at most 1024 UTF-8 bytes; empty allowed)",
    "model use",
    "none, to serve text search only",
    "url",
    "Embedding server URL, including the embeddings endpoint path",
    "model",
    "Embedding model identifier accepted by the server (1-4096 UTF-8 bytes)",
    "api_key_env",
    "Environment variable holding the embedding server credential",
    "timeout_seconds",
    "HTTP request timeout in seconds (1-120)",
    "request_dimensions",
    "Include the dimensions parameter in embedding requests",
    "Local tokenizer JSON file for exact HTTP chunk sizing (requires --max-tokens)",
    "",
    "unknown-help-key",
    "🧪",
    "\0",
];

#[test]
fn help_tables_preserve_every_literal_key_path_override_and_fallback() {
    for path in REPRESENTATIVES {
        for id in REPRESENTATIVES {
            assert_eq!(description(path, id), before(path, id), "{path:?}/{id:?}");
        }
    }
}
