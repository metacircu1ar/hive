use super::*;
use crate::runtime::{api::events, domain::ProviderEventKind};

fn last_event(store: &Store) -> (u64, String) {
    store
        .query_row_cached(
            "SELECT seq,data FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((rows::unsigned(row, 0)?, row.get(1)?)),
        )
        .unwrap()
}

#[test]
fn typed_events_keep_canonical_keys_escaping_nulls_and_omissions() {
    let store = crate::runtime::test_support::TestStore::new();
    let agent = agent("a", None);
    store.save(&agent).unwrap();
    let caller = crate::runtime::hub::Caller::Human;
    let data = events::MessageAccepted {
        attachments: &[],
        id: "id",
        sender: &caller,
        body: "é\n\"\0",
        sender_turn_id: None,
        not_before: None,
        mode: None,
        steering_target: None,
        redirect_target: None,
    };
    let expected = concat!(
        "{\"body\":\"é\\n\\\"\\u0000\",\"id\":\"id\",",
        "\"sender\":\"human\",\"sender_turn_id\":null}"
    );
    assert_eq!(api::value(&data).to_string(), expected);
    store
        .event_typed(&agent, EventKind::MessageAccepted, data)
        .unwrap();
    assert_eq!(last_event(&store).1, expected);
}

#[test]
fn typed_provider_events_keep_flattening_and_full_unsigned_precision() {
    let store = crate::runtime::test_support::TestStore::new();
    let data = events::ProviderStarted {
        provider_id: "provider",
        pgid: u32::MAX,
        process: events::ProcessStart {
            pid: u32::MAX,
            spawn_before: u64::MAX,
            spawn_after: u64::MAX,
        },
    };
    let expected = api::value(&data).to_string();
    store
        .provider_event("provider", ProviderEventKind::Started, data)
        .unwrap();
    let saved: String = store
        .query_row_cached(
            "SELECT data FROM provider_events ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, expected);
}

#[test]
fn typed_sequenced_events_return_the_sequence_that_contains_the_same_payload() {
    let store = crate::runtime::test_support::TestStore::new();
    let agent = agent("a", None);
    store.save(&agent).unwrap();
    let data = events::TurnFinished {
        turn_id: None,
        status: "failed",
        message_id: "message",
    };
    let expected = api::value(&data).to_string();
    let first = store
        .event_with_sequence(&agent, EventKind::TurnFinished, &data)
        .unwrap();
    assert_eq!(last_event(&store), (first, expected.clone()));
    let second = store
        .event_with_sequence(&agent, EventKind::TurnFinished, data)
        .unwrap();
    assert_eq!(last_event(&store), (second, expected));
    assert!(second > first);
}

#[derive(serde::Deserialize)]
struct ArtifactEnvelope {
    artifact: Artifact,
}
#[derive(serde::Deserialize)]
struct Artifact {
    id: String,
    bytes: usize,
}

#[test]
fn typed_event_publication_preserves_the_exact_inline_artifact_boundary() {
    let store = crate::runtime::test_support::TestStore::new();
    let agent = agent("a", None);
    store.save(&agent).unwrap();
    let boundary = crate::runtime::store::events::INLINE_EVENT_BYTES;
    for bytes in [boundary - 1, boundary, boundary + 1] {
        let text = "x".repeat(bytes - r#"{"text":""}"#.len());
        let data = events::TextArtifact { text: &text };
        let expected = api::value(&data);
        assert_eq!(expected.to_string().len(), bytes);
        store
            .event(&agent, EventKind::AssistantMessage, expected.clone())
            .unwrap();
        let raw = last_event(&store).1;
        store
            .event_typed(&agent, EventKind::AssistantMessage, data)
            .unwrap();
        let typed = last_event(&store).1;
        if bytes <= boundary {
            assert_eq!(typed, raw);
            assert_eq!(typed, expected.to_string());
        } else {
            for saved in [raw, typed] {
                let envelope: ArtifactEnvelope = serde_json::from_str(&saved).unwrap();
                // Published JSON artifacts include the file's terminal newline.
                assert_eq!(envelope.artifact.bytes, bytes + 1);
                let output = store.output(&envelope.artifact.id, 0, 65536).unwrap();
                assert!(output.eof);
                assert_eq!(output.text, format!("{expected}\n"));
            }
        }
    }
}
