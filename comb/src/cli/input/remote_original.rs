//! Only pointers belonging to the submitted command become CLI fields.
use comb_protocol::service::input::retrieval::Read;

pub(super) fn field(
    command: &'static str,
    pointer: &str,
    source: Option<&'static str>,
) -> Option<&'static str> {
    if command == "embed" {
        return (pointer == "/options/replace").then_some("--replace");
    }
    if command == "setup" {
        return comb::setup::input::field(pointer);
    }
    if command == "search" {
        if pointer == "/operation/request/graph" {
            return Some("--graph");
        }
        if let Some(field) = pointer.strip_prefix("/operation/request/graph/") {
            return Read::GraphQuery
                .fields()
                .contains(&field)
                .then_some("--graph");
        }
    }
    if command == "serve" {
        return match pointer {
            "/options/replace_embeddings" => Some("--replace-embeddings"),
            "/options/retry_failed" => Some("--retry-failed"),
            "/options/vector_cache_mib" => Some("--vector-cache-mib"),
            _ => None,
        };
    }
    if let Some(mutation) =
        comb_protocol::service::input::mutation::Write::named(if command == "rm" {
            "remove"
        } else {
            command
        })
    {
        return write_field(mutation, command, pointer, source);
    }
    if let Some(admin) = admin(command) {
        return admin_field(admin, command, pointer);
    }
    read_field(command, pointer)
}

fn read_field(command: &'static str, pointer: &str) -> Option<&'static str> {
    let read = Read::named(if command == "graph-query" {
        "graph_query"
    } else {
        command
    })?;
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

fn admin(command: &str) -> Option<comb_protocol::service::input::admin::Admin> {
    comb_protocol::service::input::admin::Admin::named(match command {
        "credential create" => "credential_create",
        "credential list" => "credential_list",
        "credential revoke" => "credential_revoke",
        command => command,
    })
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
