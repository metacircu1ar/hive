//! Only pointers belonging to the submitted command become CLI fields.
use comb_protocol::service::input::{admin::Admin, mutation::Write, retrieval::Read};

enum Operation {
    Embed,
    Setup,
    Serve,
    Read(Read),
    Write(Write),
    Admin(Admin),
    Unknown,
}
impl Operation {
    fn named(command: &str) -> Self {
        match command {
            "embed" => Self::Embed,
            "setup" => Self::Setup,
            "serve" => Self::Serve,
            "rm" => Self::Write(Write::Remove),
            "graph-query" => Self::Read(Read::GraphQuery),
            "credential create" => Self::Admin(Admin::CredentialCreate),
            "credential list" => Self::Admin(Admin::CredentialList),
            "credential revoke" => Self::Admin(Admin::CredentialRevoke),
            name => Write::named(name)
                .map(Self::Write)
                .or_else(|| Admin::named(name).map(Self::Admin))
                .or_else(|| Read::named(name).map(Self::Read))
                .unwrap_or(Self::Unknown),
        }
    }
}
pub(super) fn field(
    command: &'static str,
    pointer: &str,
    source: Option<&'static str>,
) -> Option<&'static str> {
    match Operation::named(command) {
        Operation::Embed => (pointer == "/options/replace").then_some("--replace"),
        Operation::Setup => comb::setup::input::field(pointer),
        Operation::Serve => match pointer {
            "/options/replace_embeddings" => Some("--replace-embeddings"),
            "/options/retry_failed" => Some("--retry-failed"),
            "/options/vector_cache_mib" => Some("--vector-cache-mib"),
            _ => None,
        },
        Operation::Read(Read::Search) if pointer == "/operation/request/graph" => Some("--graph"),
        Operation::Read(Read::Search) if pointer.starts_with("/operation/request/graph/") => {
            let field = pointer.strip_prefix("/operation/request/graph/")?;
            Read::GraphQuery
                .fields()
                .contains(&field)
                .then_some("--graph")
        }
        Operation::Write(mutation) => write_field(mutation, command, pointer, source),
        Operation::Admin(admin) => admin_field(admin, command, pointer),
        Operation::Read(read) => read_field(read, command, pointer),
        Operation::Unknown => None,
    }
}

fn read_field(read: Read, command: &'static str, pointer: &str) -> Option<&'static str> {
    if pointer == "/operation" {
        return Some(command);
    }
    if matches!(read, Read::Search | Read::List | Read::GraphQuery)
        && pointer == "/operation/request"
    {
        return Some(command);
    }
    let name = if read == Read::Search && pointer == "/operation/mode" {
        "mode"
    } else if matches!(read, Read::Search | Read::List | Read::GraphQuery) {
        pointer.strip_prefix("/operation/request/")?
    } else {
        pointer.strip_prefix("/operation/")?
    };
    if !read.fields().contains(&name) {
        return None;
    }
    match name {
        "slug" => Some("SLUG"),
        "anchor" => Some("ANCHOR"),
        "to" => Some("--to"),
        "predicates" => Some("--predicate"),
        "direction" => Some("--direction"),
        "reviewed_only" => Some("--reviewed-only"),
        "strict" => Some("--strict"),
        "max_nodes" => Some("--max-nodes"),
        "max_edges" => Some("--max-edges"),
        "max_evidence_bytes" => Some("--max-evidence-bytes"),
        "deadline_ms" => Some("--deadline-ms"),
        "query" => Some("QUERY"),
        "limit" => Some("--limit"),
        "depth" => Some("--depth"),
        "mode" if pointer == "/operation/mode" => Some("--mode"),
        "kind" => Some("--type"),
        "tag" => Some("--tag"),
        "prefix" => Some("--prefix"),
        "after" => Some("--after"),
        "before" => Some("--before"),
        "unverified" => Some("--unverified"),
        _ => None,
    }
}

fn write_field(
    mutation: comb_protocol::service::input::mutation::Write,
    command: &'static str,
    pointer: &str,
    source: Option<&'static str>,
) -> Option<&'static str> {
    use comb_protocol::service::input::mutation::Write;
    if pointer == "/operation"
        || (mutation == Write::Consolidate && pointer == "/operation/request")
    {
        return Some(command);
    }
    let field = mutation
        .fields()
        .iter()
        .find(|field| mutation.pointer(field, false) == pointer)?;
    match *field {
        "slug" => Some("SLUG"),
        "note" => Some("NOTE"),
        "target" => Some("TARGET"),
        "markdown" => source,
        "text" => Some("TEXT"),
        "source" => Some("--source"),
        "summary" => Some("--summary-file"),
        "expected_hash" => Some("--expected-hash"),
        "note_hash" => Some("--note-hash"),
        "target_hash" => Some("--target-hash"),
        "request_id" => Some("--request-id"),
        "run" => Some("--run"),
        "decision" => Some("DECISION"),
        "reason" => Some("--reason"),
        "recopy" => Some("--recopy"),
        _ => None,
    }
}

fn admin_field(
    admin: comb_protocol::service::input::admin::Admin,
    command: &'static str,
    pointer: &str,
) -> Option<&'static str> {
    use comb_protocol::service::input::admin::Admin;
    if pointer == "/operation" {
        return Some(command);
    }
    let field = pointer.strip_prefix("/operation/")?;
    match (admin, field) {
        (Admin::Import, "directory") => Some("DIR"),
        (Admin::Import, "prefix") => Some("--prefix"),
        (Admin::CredentialRevoke, "id") => Some("ID"),
        (Admin::CredentialCreate, "grant") => Some(command),
        (Admin::CredentialCreate, "grant/name") => Some("NAME"),
        (Admin::CredentialCreate, "grant/kind") => Some("--bridge"),
        (Admin::CredentialCreate, "grant/reviewer") => Some("--reviewer"),
        (Admin::CredentialCreate, "grant/reviewer_roles") => Some("--reviewer-role"),
        (Admin::CredentialCreate, field) => {
            let index = field.strip_prefix("grant/reviewer_roles/")?;
            let number = index.parse::<usize>().ok()?;
            (number < comb_protocol::service::limits::REVIEWER_ROLES && number.to_string() == index)
                .then_some("--reviewer-role")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::field;
    #[test]
    fn administrative_fields_are_command_scoped_and_role_indices_bounded() {
        for (pointer, expected) in [
            ("/operation/grant/name", Some("NAME")),
            ("/operation/grant/kind", Some("--bridge")),
            ("/operation/grant/reviewer", Some("--reviewer")),
            ("/operation/grant/reviewer_roles/0", Some("--reviewer-role")),
            (
                "/operation/grant/reviewer_roles/63",
                Some("--reviewer-role"),
            ),
            ("/operation/grant/reviewer_roles/64", None),
            ("/operation/grant/reviewer_roles/00", None),
            ("/operation/grant/reviewer_roles/+1", None),
            ("/operation/grant/reviewer_roles/1/private", None),
            ("/operation/id", None),
        ] {
            assert_eq!(field("credential create", pointer, None), expected);
        }
        assert_eq!(
            field("credential revoke", "/operation/id", None),
            Some("ID")
        );
        assert_eq!(field("credential list", "/operation/id", None), None);
        assert_eq!(field("import", "/operation/directory", None), Some("DIR"));
        assert_eq!(field("import", "/operation/grant/name", None), None);
        assert_eq!(field("health", "/operation", None), Some("health"));
        assert_eq!(field("health", "/operation/grant", None), None);
    }
    #[test]
    fn consolidation_nested_fields_and_remove_use_only_their_own_coordinates() {
        for (pointer, expected) in [
            ("/operation/request", Some("consolidate")),
            ("/operation/request/note", Some("NOTE")),
            ("/operation/request/target", Some("TARGET")),
            ("/operation/request/note_hash", Some("--note-hash")),
            ("/operation/request/target_hash", Some("--target-hash")),
            ("/operation/request/summary", Some("--summary-file")),
            ("/operation/request/recopy", Some("--recopy")),
            ("/operation/request_id", Some("--request-id")),
            ("/operation/run", Some("--run")),
            ("/operation/note", None),
            ("/operation/request/request_id", None),
            ("/operation/slug", None),
        ] {
            assert_eq!(field("consolidate", pointer, None), expected);
        }
        assert_eq!(
            field("rm", "/operation/expected_hash", None),
            Some("--expected-hash")
        );
        assert_eq!(field("rm", "/operation/run", None), None);
        assert_eq!(field("capture", "/operation/slug", None), None);
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    #[test]
    fn typed_dispatch_preserves_all_cli_protocol_and_unknown_labels() {
        let commands = [
            "embed",
            "setup",
            "serve",
            "rm",
            "remove",
            "graph-query",
            "graph_query",
            "credential create",
            "credential list",
            "credential revoke",
            "credential_create",
            "credential_list",
            "credential_revoke",
            "import",
            "import_status",
            "import_abandon",
            "health",
            "capabilities",
            "stats",
            "doctor",
            "sync",
            "put",
            "append",
            "capture",
            "consolidate",
            "decide",
            "promote",
            "search",
            "get",
            "list",
            "links",
            "backlinks",
            "graph",
            "unknown",
            "SEARCH",
        ];
        for command in commands {
            let operation = Operation::named(command);
            let fields = match operation {
                Operation::Read(read) => read.fields(),
                Operation::Write(mutation) => mutation.fields(),
                Operation::Admin(admin) => admin.fields(),
                _ => &[],
            };
            for field in fields {
                for pointer in [
                    format!("/operation/{field}"),
                    format!("/operation/request/{field}"),
                ] {
                    compare(command, &pointer);
                }
            }
            for pointer in [
                "/operation",
                "/operation/request",
                "/operation/mode",
                "/options/replace",
                "/options/replace_embeddings",
                "/options/retry_failed",
                "/options/vector_cache_mib",
                "/operation/request/graph",
                "/operation/request/graph/anchor",
                "/operation/request/graph/deadline_ms",
                "/operation/request/graph/unknown",
                "/operation/grant/name",
                "/operation/grant/kind",
                "/operation/grant/reviewer",
                "/operation/grant/reviewer_roles/0",
                "/operation/grant/reviewer_roles/63",
                "/operation/grant/reviewer_roles/64",
                "/operation/grant/reviewer_roles/01",
                "/operation/grant/reviewer_roles/-1",
                "/unknown",
            ] {
                compare(command, pointer);
            }
        }
    }
    fn compare(command: &'static str, pointer: &str) {
        for source in [None, Some("--file"), Some("TEXT")] {
            assert_eq!(
                field(command, pointer, source),
                crate::cli::input::remote_original::field(command, pointer, source),
                "{command} {pointer} {source:?}"
            );
        }
    }
}
