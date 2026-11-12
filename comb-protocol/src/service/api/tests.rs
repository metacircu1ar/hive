use super::*;

// Current operations with inline Put/Append fields are the extraction oracle.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum InlineOperation {
    Capabilities,
    /// Aggregate setup diagnostics; no page contents or agent attribution.
    Health,
    Get {
        slug: Slug,
    },
    Search {
        request: Search,
        #[serde(default)]
        mode: Mode,
    },
    List {
        request: List,
    },
    Links {
        slug: Slug,
        #[serde(default)]
        incoming: bool,
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "link_limit")]
        limit: usize,
    },
    Graph {
        slug: Slug,
        depth: usize,
    },
    GraphQuery {
        request: crate::graph::Query,
    },
    Stats,
    Doctor,
    Put {
        slug: Slug,
        markdown: String,
        #[serde(default)]
        expected_hash: Option<ContentHash>,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Append {
        slug: Slug,
        text: String,
        source: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        expected_hash: Option<ContentHash>,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Capture {
        text: String,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Promote {
        slug: Slug,
        expected_hash: ContentHash,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Consolidate {
        request: Consolidate,
        request_id: RequestId,
        run: RunName,
    },
    Decide {
        slug: Slug,
        expected_hash: ContentHash,
        decision: crate::page::Decision,
        reason: String,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Remove {
        slug: Slug,
        expected_hash: ContentHash,
        request_id: RequestId,
    },
    Sync,
    Import {
        directory: PathBuf,
        prefix: Option<Slug>,
    },
    ImportStatus,
    ImportAbandon {
        batch_id: ImportId,
    },
    CredentialCreate {
        grant: Grant,
    },
    CredentialList,
    CredentialRevoke {
        id: CredentialId,
    },
}

fn same_contract(raw: &str) {
    let before = serde_json::from_str::<InlineOperation>(raw);
    let after = serde_json::from_str::<Operation>(raw);
    match (before, after) {
        (Ok(before), Ok(after)) => assert_eq!(
            serde_json::to_vec(&before).unwrap(),
            serde_json::to_vec(&after).unwrap(),
            "{raw}"
        ),
        (Err(before), Err(after)) => assert_eq!(before.to_string(), after.to_string(), "{raw}"),
        _ => panic!("Mutation payload changed acceptance: {raw}"),
    }
}

#[test]
fn mutation_payloads_preserve_inline_wire_bytes_and_strict_decoding() {
    for raw in [
        r#"{"op":"put","slug":"components/writer","markdown":"hello\nworld","request_id":"create"}"#,
        r#"{"op":"append","slug":"components/writer","text":"hello","source":"test","request_id":"append"}"#,
    ] {
        same_contract(raw);
        let original: serde_json::Map<String, Value> = serde_json::from_str(raw).unwrap();
        for field in original.keys() {
            let mut omitted = original.clone();
            omitted.remove(field);
            same_contract(&serde_json::to_string(&omitted).unwrap());
        }
        let fields: Vec<_> = original
            .keys()
            .map(String::as_str)
            .chain(["expected_hash", "run", "summary", "unknown"])
            .collect();
        for field in fields {
            for value in ["null", "false", "17", "[]", "{}", r#""value""#] {
                let mut altered = original.clone();
                altered.insert(field.into(), serde_json::from_str(value).unwrap());
                same_contract(&serde_json::to_string(&altered).unwrap());
                let duplicate = format!("{{\"{field}\":{value},{}", &raw[1..]);
                same_contract(&duplicate);
            }
        }
    }
}
