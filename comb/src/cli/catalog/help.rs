//! Help for CLI-owned fields and imported options lacking their own description.
use clap::{Command, ValueHint};

pub(super) fn arguments(mut command: Command, path: &str) -> Command {
    let missing: Vec<_> = command
        .get_arguments()
        .filter(|arg| arg.get_help().is_none())
        .filter_map(|arg| {
            description(path, arg.get_id().as_str()).map(|help| (arg.get_id().clone(), help))
        })
        .collect();
    for (id, help) in missing {
        command = command.mut_arg(id, |arg| arg.help(help));
    }
    let paths: Vec<_> = command
        .get_arguments()
        .filter(|arg| {
            matches!(
                arg.get_id().as_str(),
                "comb"
                    | "socket"
                    | "credential"
                    | "file"
                    | "summary_file"
                    | "config"
                    | "embedding_config"
                    | "out"
                    | "tokenizer"
                    | "llama_server"
                    | "dir"
            )
        })
        .map(|arg| arg.get_id().clone())
        .collect();
    for id in paths {
        let directory = matches!(id.as_str(), "comb" | "dir");
        command = command.mut_arg(id, |arg| {
            arg.value_hint(if directory {
                ValueHint::DirPath
            } else {
                ValueHint::FilePath
            })
        });
    }
    if command.get_arguments().any(|arg| arg.get_id() == "comb") {
        command = command.mut_arg("comb", |arg| arg.value_name("ROOT"));
    }
    if command.get_arguments().any(|arg| arg.get_id() == "kind") {
        command = command.mut_arg("kind", |arg| arg.value_name("TYPE"));
    }
    command
}

const GENERAL_HELP: &[(&str, &str)] = &[
    (
        "comb",
        "Knowledge root (except version/mcp; current directory, setup/service: $HOME/Comb)",
    ),
    (
        "socket",
        "Service socket for setup, serve, mcp and remote commands (not sync --rebuild)",
    ),
    (
        "run",
        "Run name for put, append, capture, promote, consolidate and decide",
    ),
    ("slug", "Knowledge page slug, for example components/writer"),
    (
        "expected_hash",
        "Current page hash required for the intended edit",
    ),
    (
        "request_id",
        "Stable caller request ID; reuse it with identical input when retrying",
    ),
    (
        "summary_file",
        "UTF-8 replacement summary (append also requires --expected-hash)",
    ),
    ("kind", "Filter by page metadata type"),
    ("tag", "Filter by one page tag"),
    (
        "prefix",
        "Filter by literal slug prefix, for example components/",
    ),
    ("unverified", "Return only pages with unverified content"),
    ("after", "Continue after this page slug"),
    ("limit", "Maximum rows in this page (1-1000)"),
    ("query", "Search query text"),
    (
        "mode",
        "Literal text, semantic vector, or combined hybrid retrieval",
    ),
    ("embedding_config", "Embedding configuration file"),
    (
        "depth",
        "Maximum graph distance from the starting page (0-8)",
    ),
    (
        "config",
        "Embedding configuration file for offline indexing",
    ),
    ("replace", "Replace the currently active embedding space"),
    (
        "replace_embeddings",
        "Allow this server to replace the active embedding space",
    ),
    (
        "retry_failed",
        "Retry chunks previously rejected during indexing",
    ),
    (
        "vector_cache_mib",
        "Maximum in-memory vector cache size in MiB (1-8192; default 512 with a model)",
    ),
    (
        "rebuild",
        "Rebuild the derived text index while retaining durable knowledge",
    ),
    (
        "reviewer_role",
        "Hive role allowed to review through the bridge (repeat for multiple roles)",
    ),
    (
        "bridge_file",
        "Private Hive bridge connection file to create or reuse",
    ),
];

const SPECIFIC_HELP: &[(&str, &str, &str)] = &[
    (
        "version",
        "json",
        "Print the installed binary identity as JSON",
    ),
    (
        "mcp",
        "credential",
        "Private client connection file containing the socket and credential",
    ),
    (
        "init",
        "dir",
        "Knowledge directory to initialize (defaults to --root)",
    ),
    (
        "import",
        "dir",
        "Directory of Markdown files to import or resume",
    ),
    (
        "import",
        "status",
        "Show the retained import batch and confirmed progress without reading its source",
    ),
    (
        "import",
        "abandon",
        "Abandon this exact import batch ID, preserving completed pages and receipts",
    ),
    (
        "put",
        "file",
        "UTF-8 Markdown file to read (stdin when omitted)",
    ),
    ("append", "text", "Evidence text to append"),
    ("capture", "text", "Inbox note text"),
    (
        "append",
        "source",
        "Evidence source assertion, for example commit:abc123",
    ),
    ("consolidate", "note", "Inbox note slug to consolidate"),
    (
        "consolidate",
        "target",
        "Existing ordinary page receiving the note's evidence",
    ),
    ("consolidate", "note_hash", "Current hash of the inbox note"),
    (
        "consolidate",
        "target_hash",
        "Current hash of the target page",
    ),
    (
        "decide",
        "decision",
        "Whether to accept or reject the proposal",
    ),
    (
        "decide",
        "reason",
        "Explanation recorded with the review decision",
    ),
    (
        "import",
        "prefix",
        "Slug prefix for imported pages, for example operations",
    ),
    (
        "search",
        "after",
        "Include pages updated at or after this RFC 3339 timestamp",
    ),
    (
        "graph-query",
        "after",
        "Include pages updated at or after this RFC 3339 timestamp",
    ),
    (
        "search",
        "before",
        "Include pages updated at or before this RFC 3339 timestamp",
    ),
    (
        "graph-query",
        "before",
        "Include pages updated at or before this RFC 3339 timestamp",
    ),
    ("search", "limit", "Maximum result pages (1-100)"),
    (
        "credential create",
        "name",
        "Human-readable label for this client credential",
    ),
    (
        "credential create",
        "out",
        "New private connection file to write",
    ),
    (
        "credential create",
        "bridge",
        "Issue a Hive bridge credential",
    ),
    (
        "credential create",
        "reviewer",
        "Allow this client to review knowledge",
    ),
    (
        "credential revoke",
        "id",
        "UUID of the credential to revoke",
    ),
];

fn description(path: &str, id: &str) -> Option<&'static str> {
    if path == "graph-query"
        && let Some(help) = graph(id)
    {
        return Some(help);
    }
    SPECIFIC_HELP
        .iter()
        .find_map(|(command, field, text)| (*command == path && *field == id).then_some(*text))
        .or_else(|| {
            GENERAL_HELP
                .iter()
                .find_map(|(field, text)| (*field == id).then_some(*text))
        })
        .or_else(|| model(path, id))
}

fn graph(id: &str) -> Option<&'static str> {
    Some(match id {
        "to" => "Destination page slug for one shortest connection",
        "direction" => "Follow outgoing, incoming or both directions",
        "depth" => "Maximum relation distance from the starting page (0-4)",
        "limit" => "Maximum returned relations (1-200)",
        "max_nodes" => "Maximum nodes visited across the query (1-1000)",
        "max_edges" => "Maximum edges inspected across the query (1-5000)",
        "max_evidence_bytes" => "Maximum evidence bytes returned across the query (1-262144)",
        "deadline_ms" => "Query deadline in milliseconds (1-500)",
        _ => return None,
    })
}

fn model(path: &str, id: &str) -> Option<&'static str> {
    Some(match (path, id) {
        ("model use-http", "dimensions") => "Embedding vector dimension count (1-4096)",
        ("model use-http", "max_tokens") => {
            "Maximum tokenizer input length (16-8192; requires --tokenizer)"
        }
        ("model use-http", "query_prefix" | "document_prefix") => {
            "Text prepended to embeddings (at most 1024 UTF-8 bytes; empty allowed)"
        }
        ("model use", "name") => {
            "none, to serve text search only, or qwen3-local for local embeddings"
        }
        (_, "url") => "Embedding server URL, including the embeddings endpoint path",
        ("model use-http", "model") => {
            "Embedding model identifier accepted by the server (1-4096 UTF-8 bytes)"
        }
        (_, "api_key_env") => "Environment variable holding the embedding server credential",
        (_, "timeout_seconds") => "HTTP request timeout in seconds (1-120)",
        (_, "request_dimensions") => "Include the dimensions parameter in embedding requests",
        (_, "tokenizer") => {
            "Local tokenizer JSON file for exact HTTP chunk sizing (requires --max-tokens)"
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
