use super::*;
use crate::{ErrorCode, service::auth::Credential, values::CombId};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use tokio::io::AsyncWriteExt as _;

async fn exchange(reply: Value) -> Error {
    exchange_bytes(&serde_json::to_vec(&reply).unwrap()).await
}
async fn exchange_bytes(reply: &[u8]) -> Error {
    exchange_operation(
        reply,
        Operation::Capture {
            text: "fixture note".into(),
            request_id: "capture-fixture".to_owned().try_into().unwrap(),
            run: None,
        },
    )
    .await
    .unwrap_err()
}
async fn exchange_operation(reply: &[u8], operation: Operation) -> Result<Value> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.path().canonicalize().unwrap().join("comb.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let connection = Connection {
        format: hive_input::compatibility::Exact,
        socket,
        credential: Credential {
            comb_id: CombId::generate(),
            id: "human".to_owned().try_into().unwrap(),
            secret: "fixture-secret".into(),
        },
    };
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let bytes = framing::read(&mut stream, framing::REQUEST_BYTES)
            .await
            .unwrap()
            .unwrap();
        let request = super::super::input::request(&bytes).unwrap();
        assert_eq!(request.protocol, 1);
        assert_eq!(
            serde_json::to_value(request.operation).unwrap(),
            serde_json::to_value(&operation).unwrap()
        );
        stream.get_mut().write_all(reply).await.unwrap();
        stream.get_mut().write_all(b"\n").await.unwrap();
    };
    let submitted = std::sync::atomic::AtomicBool::new(false);
    let (error, ()) = tokio::join!(
        call_observed(
            &connection,
            &operation,
            None,
            Duration::from_secs(2),
            || submitted.store(true, std::sync::atomic::Ordering::Relaxed)
        ),
        server
    );
    assert!(submitted.load(std::sync::atomic::Ordering::Relaxed));
    error
}

#[tokio::test]
async fn m5c_submission_observer_does_not_mark_missing_socket_or_rejected_frame() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.path().canonicalize().unwrap().join("comb.sock");
    let connection = Connection {
        format: hive_input::compatibility::Exact,
        socket: socket.clone(),
        credential: Credential {
            comb_id: CombId::generate(),
            id: "human".to_owned().try_into().unwrap(),
            secret: "fixture".into(),
        },
    };
    for oversized in [false, true] {
        let listener = oversized.then(|| tokio::net::UnixListener::bind(&socket).unwrap());
        if oversized {
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let submitted = std::sync::atomic::AtomicBool::new(false);
        let operation = Operation::Capture {
            text: "x".repeat(if oversized { framing::REQUEST_BYTES } else { 1 }),
            request_id: "capture".to_owned().try_into().unwrap(),
            run: None,
        };
        let error = call_observed(
            &connection,
            &operation,
            None,
            Duration::from_secs(1),
            || submitted.store(true, std::sync::atomic::Ordering::Relaxed),
        )
        .await
        .unwrap_err();
        assert!(!submitted.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!error.file_committed);
        drop(listener);
    }
}

#[tokio::test]
async fn submitted_health_exchange_rejects_a_positional_result() {
    let bytes = br#"{"protocol":1,"status":"ok","result":[null,"0.1.0",[],null,0,0,0,0]}"#;
    let error = exchange_operation(bytes, Operation::Health)
        .await
        .unwrap_err();
    assert_eq!(error.code, Operation::Health.lost_reply().code);
    assert!(!error.file_committed);
    assert!(error.request_id.is_none());
}

#[tokio::test]
async fn duplicate_result_keys_and_positional_replies_keep_submitted_mutations_uncertain() {
    for reply in [
        r#"{"protocol":1,"status":"ok","result":{"secret":"first","secret":"second"}}"#,
        r#"{"protocol":1,"status":"ok","result":{"nested":[{"x":1,"x":2}]}}"#,
        r#"{"protocol":1,"status":"ok","result":[1,2]}"#,
        r#"["ok",1,{"ready":true}]"#,
    ] {
        let error = exchange_bytes(reply.as_bytes()).await;
        assert_eq!(error.code, ErrorCode::Uncertain);
        assert!(error.file_committed);
        assert_eq!(error.request_id.as_deref(), Some("capture-fixture"));
    }
}

#[test]
fn result_validation_preserves_declared_arrays_and_requires_exact_health_objects() {
    let graph = Operation::Graph {
        slug: "notes/one".parse().unwrap(),
        depth: 1,
    };
    result_shape(&graph, &json!(["notes/two"])).unwrap();
    result_shape(
        &Operation::CredentialList,
        &json!([{"id":"human","grant":{"kind":"client","name":"test"}}]),
    )
    .unwrap();
    assert!(result_shape(&Operation::CredentialList, &json!([["human"]])).is_err());
    assert!(result_shape(&graph, &json!([{"slug":"notes/two"}])).is_err());
    let health = json!({"product_version":"0.1.0","compiled_features":[],"model":null,
        "total":0,"embedded":0,"pending":0,"rejected":0});
    result_shape(&Operation::Health, &health).unwrap();
    let mut missing = health.clone();
    missing.as_object_mut().unwrap().remove("model");
    assert!(result_shape(&Operation::Health, &missing).is_err());
    let mut unknown = health.clone();
    unknown["unknown"] = true.into();
    assert!(result_shape(&Operation::Health, &unknown).is_err());
    let mut nested = health;
    nested["model"] = json!(["space", "model", "revision", "http", 8, true]);
    assert!(result_shape(&Operation::Health, &nested).is_err());
}
fn future_error(committed: bool) -> Value {
    json!({"protocol":1,"status":"error","error":{
        "code":"future_failure","message":"Future peer error.",
        "file_committed":committed,"request_id":"capture-fixture",
        "import_progress":{"batch_id":"00000000-0000-4000-8000-000000000001",
            "state":"pending","total":3,"completed":1,"unfinished":2,"unresolved":"notes/one"}
    }})
}
#[tokio::test]
async fn unknown_errors_preserve_acceptance_identity_and_progress() {
    for committed in [false, true] {
        let error = exchange(future_error(committed)).await;
        assert_eq!(error.code, ErrorCode::UnknownError);
        assert_eq!(error.raw_code(), Some("future_failure"));
        let encoded = serde_json::to_value(&error).unwrap();
        assert_eq!(encoded["code"], "unknown_error");
        assert_eq!(encoded["raw_code"], "future_failure");
        let roundtrip: Error = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(roundtrip.raw_code(), error.raw_code());
        assert_eq!(serde_json::to_value(roundtrip).unwrap(), encoded);
        assert_eq!(error.file_committed, committed);
        assert_eq!(error.request_id.as_deref(), Some("capture-fixture"));
        let progress = error.import_progress.unwrap();
        assert_eq!(progress.total, 3);
        assert_eq!(progress.completed, 1);
        assert_eq!(progress.unfinished, 2);
    }
}
#[tokio::test]
async fn malformed_replies_after_submission_remain_uncertain_without_retry() {
    let original = future_error(false);
    let mut cases = Vec::new();
    for (key, value) in [("protocol", json!(2)), ("extra", json!(true))] {
        let mut reply = original.clone();
        reply[key] = value;
        cases.push(reply);
    }
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("protocol");
    cases.push(missing);
    for (key, value) in [
        ("file_committed", json!("false")),
        ("request_id", json!("")),
        ("code", json!("invalid code")),
        ("import_progress", json!(null)),
    ] {
        let mut reply = original.clone();
        reply["error"][key] = value;
        cases.push(reply);
    }
    let mut progress = original;
    progress["error"]["import_progress"]["unfinished"] = json!(0);
    cases.push(progress);
    for reply in cases {
        let error = exchange(reply).await;
        assert_eq!(error.code, ErrorCode::Uncertain, "{error:?}");
        assert!(error.file_committed);
        assert_eq!(error.request_id.as_deref(), Some("capture-fixture"));
    }
}

#[test]
fn compatibility_service_contract() {
    let original: Value =
        serde_json::from_str(include_str!("../../compatibility/service-envelope.json")).unwrap();
    let request =
        super::super::input::request(&serde_json::to_vec(&original["request"]).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), original["request"]);
    assert_eq!(
        serde_json::to_value(Reply::from_result(Ok(json!({"ready":true})))).unwrap(),
        original["reply"]
    );
    for protocol in [None, Some(json!(2)), Some(json!(null))] {
        let mut value = original["request"].clone();
        value.as_object_mut().unwrap().remove("protocol");
        if let Some(protocol) = protocol {
            value["protocol"] = protocol;
        }
        assert!(super::super::input::request(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let positional = json!([1,original["request"]["credential"],null,{"op":"health"}]);
    assert!(super::super::input::request(&serde_json::to_vec(&positional).unwrap()).is_err());
}
