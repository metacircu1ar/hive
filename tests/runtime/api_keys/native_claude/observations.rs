//! Public-wire projections for sanitized native evidence, without private Rust APIs.
use super::*;
use crate::harness::views::agent::FinishedTurn;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Counters {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    web_search_requests: Option<u64>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Usage {
    pub session_id: String,
    pub turn_id: String,
    observed_at: u64,
    pub outcome: String,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    reset: Option<bool>,
    pub models: BTreeMap<String, Counters>,
}
impl Usage {
    pub(super) fn current(&self, thread: Option<&str>, finished: Option<&FinishedTurn>) -> bool {
        Some(self.session_id.as_str()) == thread
            && finished.is_some_and(|finished| {
                self.session_id == finished.thread
                    && self.turn_id == finished.turn
                    && self.outcome == "completed"
                    && finished.outcome == "completed"
                    && finished.message.is_some()
            })
    }
}

#[test]
fn usage_reset_is_required_but_nullable() {
    let mut value = json!({"session_id":"s","turn_id":"t","observed_at":1,
        "outcome":"completed","models":{}});
    assert!(serde_json::from_value::<Usage>(value.clone()).is_err());
    for reset in [Value::Null, json!(false), json!(true)] {
        value["reset"] = reset.clone();
        let usage: Usage = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(usage).unwrap()["reset"], reset);
    }
}

#[test]
fn a_retained_snapshot_cannot_be_recorded_as_fresh_resume_usage() {
    let usage: Usage = serde_json::from_value(json!({"session_id":"session","turn_id":"old",
        "observed_at":1,"outcome":"completed","reset":false,"models":{}}))
    .unwrap();
    let mut finished = FinishedTurn {
        thread: "session".into(),
        execution: "execution".into(),
        turn: "old".into(),
        outcome: "completed".into(),
        message: Some("message".into()),
        at: 1,
    };
    assert!(usage.current(Some("session"), Some(&finished)));
    finished.turn = "resumed".into();
    assert!(!usage.current(Some("session"), Some(&finished)));
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    User,
    Assistant,
    Progress,
    System,
    Attachment,
    #[default]
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
struct Header {
    #[serde(default, rename = "type")]
    kind: Kind,
    uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    parent: Option<String>,
    #[serde(default, rename = "isSidechain")]
    sidechain: bool,
}
#[derive(Serialize)]
pub(super) struct History {
    session_id: String,
    total_rows: usize,
    main_dialogue_rows: usize,
    sidechain_rows: usize,
    nonlinear_dialogue_parents: usize,
    traversed_metadata: BTreeMap<&'static str, usize>,
}
pub(super) fn history(path: &Path, session: &str) -> History {
    let text = fs::read_to_string(path).unwrap();
    assert!(text.len() <= 64 * 1024 * 1024);
    summarize(&text, session)
}
fn summarize(text: &str, session: &str) -> History {
    let rows: Vec<Header> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(rows.len() <= 100_000);
    let links: BTreeMap<_, _> = rows
        .iter()
        .filter(|row| {
            !row.sidechain && matches!(row.kind, Kind::Progress | Kind::System | Kind::Attachment)
        })
        .filter_map(|row| Some((row.uuid.as_deref()?, row)))
        .collect();
    let mut summary = History {
        session_id: session.into(),
        total_rows: rows.len(),
        main_dialogue_rows: 0,
        sidechain_rows: 0,
        nonlinear_dialogue_parents: 0,
        traversed_metadata: BTreeMap::new(),
    };
    let mut previous = None;
    let mut dialogue = BTreeSet::new();
    for row in &rows {
        if row.sidechain {
            summary.sidechain_rows += 1;
            continue;
        }
        if !matches!(row.kind, Kind::User | Kind::Assistant) {
            continue;
        }
        summary.main_dialogue_rows += 1;
        let mut parent = row.parent.as_deref();
        let mut seen = BTreeSet::new();
        // This sanitized report describes rooted physical ancestry, including
        // parallel results. It is not the product's inheritance proof: the
        // native request assertions separately require every cohort result.
        while parent.is_some_and(|id| !dialogue.contains(id)) {
            let id = parent.unwrap();
            assert!(seen.insert(id), "native metadata cycle");
            let link = links
                .get(id)
                .expect("native main link must be known metadata");
            let kind = match link.kind {
                Kind::Progress => "progress",
                Kind::System => "system",
                Kind::Attachment => "attachment",
                _ => unreachable!(),
            };
            *summary.traversed_metadata.entry(kind).or_default() += 1;
            parent = link.parent.as_deref();
        }
        assert!(
            dialogue.is_empty() || parent.is_some(),
            "native main graph must have one root"
        );
        if parent != previous {
            summary.nonlinear_dialogue_parents += 1;
        }
        let id = row.uuid.as_deref().expect("native dialogue UUID");
        assert!(dialogue.insert(id), "duplicate native dialogue UUID");
        previous = Some(id);
    }
    summary
}

#[test]
fn native_history_evidence_records_link_types_and_counts_without_text() {
    let summary = summarize(
        concat!(
            "{\"type\":\"user\",\"uuid\":\"one\",\"parentUuid\":null}\n",
            "{\"type\":\"progress\",\"uuid\":\"progress\",\"parentUuid\":\"one\",\"data\":\"P\
                RIVATE\"}\n",
            "{\"type\":\"assistant\",\"uuid\":\"two\",\"parentUuid\":\"progress\"}\n",
            "{\"type\":\"assistant\",\"isSidechain\":true,\"uuid\":\"child\",\"parentUuid\":\
                \"one\"}\n"
        ),
        "session",
    );
    assert_eq!(summary.total_rows, 4);
    assert_eq!(summary.main_dialogue_rows, 2);
    assert_eq!(summary.sidechain_rows, 1);
    assert_eq!(summary.traversed_metadata["progress"], 1);
    assert!(!serde_json::to_string(&summary).unwrap().contains("PRIVATE"));
}

#[test]
fn native_history_summary_counts_attachment_links_and_parallel_parents() {
    let summary = summarize(
        concat!(
            "{\"type\":\"user\",\"uuid\":\"root\",\"parentUuid\":null}\n",
            "{\"type\":\"attachment\",\"uuid\":\"note\",\"parentUuid\":\"root\"}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a\",\"parentUuid\":\"note\"}\n",
            "{\"type\":\"assistant\",\"uuid\":\"b\",\"parentUuid\":\"a\"}\n",
            "{\"type\":\"user\",\"uuid\":\"rb\",\"parentUuid\":\"b\"}\n",
            "{\"type\":\"user\",\"uuid\":\"ra\",\"parentUuid\":\"a\"}\n"
        ),
        "session",
    );
    assert_eq!(summary.main_dialogue_rows, 5);
    assert_eq!(summary.nonlinear_dialogue_parents, 1);
    assert_eq!(summary.traversed_metadata["attachment"], 1);
}

#[test]
#[should_panic(expected = "native main graph must have one root")]
fn native_history_summary_refuses_a_disconnected_second_root() {
    summarize(
        "{\"type\":\"user\",\"uuid\":\"one\",\"parentUuid\":null}\n\
         {\"type\":\"assistant\",\"uuid\":\"two\",\"parentUuid\":null}\n",
        "session",
    );
}

#[test]
#[should_panic(expected = "native metadata cycle")]
fn native_history_summary_refuses_metadata_cycles() {
    summarize(
        "{\"type\":\"user\",\"uuid\":\"one\",\"parentUuid\":null}\n\
         {\"type\":\"attachment\",\"uuid\":\"a\",\"parentUuid\":\"b\"}\n\
         {\"type\":\"progress\",\"uuid\":\"b\",\"parentUuid\":\"a\"}\n\
         {\"type\":\"assistant\",\"uuid\":\"two\",\"parentUuid\":\"a\"}\n",
        "session",
    );
}
