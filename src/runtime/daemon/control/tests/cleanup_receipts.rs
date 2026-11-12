use super::*;
use crate::runtime::test_support::RuntimeFixture;
use serde_json::json;

#[tokio::test]
async fn failed_gc_and_purge_keep_receipts_for_exactly_the_original_mutating_cases() {
    let runtime = RuntimeFixture::new();
    let shutdown = runtime
        .hub
        .with(|core| Ok(core.shutdown().clone()))
        .await
        .unwrap();
    let started = Reader::start(
        crate::runtime::store::Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite"))
            .unwrap(),
        runtime.hub.published.clone(),
        shutdown,
        Arc::default(),
        runtime.hub.wake.clone(),
    );
    let pool = runtime.pool();
    let client = ClientRequest {
        shared: &runtime.hub,
        reader: &started.reader,
        pool: &pool,
    };
    for op in ["gc", "purge"] {
        for (index, dry) in [
            None,
            Some(Value::Null),
            Some(json!(false)),
            Some(json!(true)),
            Some(json!(7)),
            Some(json!("true")),
        ]
        .into_iter()
        .enumerate()
        {
            let mut args = if op == "gc" {
                json!({"op":"gc","before":u64::MAX})
            } else {
                json!({"op":"purge","target":"unknown"})
            };
            if let Some(dry) = &dry {
                args["dry_run"] = dry.clone();
            }
            let id = format!("receipt-{op}-{index}");
            let Handled { result, request_id } =
                handle(&client, json!({"protocol":1,"id":id,"args":args})).await;
            assert!(result.is_err());
            let expected = (dry != Some(json!(true))).then_some(id.as_str());
            assert_eq!(request_id.as_deref(), expected);
            let response = wire::encode(result, request_id);
            assert_eq!(response.get("request_id").and_then(Value::as_str), expected);
            if expected.is_none() {
                assert!(response.get("request_id").is_none());
            }
        }
    }
    let handled = handle(&client, json!({"protocol":1,"id":7,"args":{"op":"gc"}})).await;
    assert!(handled.result.is_err());
    assert!(handled.request_id.is_none());
    drop(pool);
    drop(started.reader);
    started.thread.join().unwrap().unwrap();
    runtime.finish();
}

async fn handle(client: &ClientRequest<'_>, raw: Value) -> Handled {
    use tokio::io::AsyncWriteExt;
    let (mut writer, reader) = tokio::net::UnixStream::pair().unwrap();
    writer
        .write_all(format!("{raw}\n").as_bytes())
        .await
        .unwrap();
    let (read, _write) = reader.into_split();
    client.read(read).await
}

#[test]
fn read_only_and_special_requests_do_not_acquire_cleanup_receipts_or_rewrite_raw_input() {
    for raw in [
        json!({"op":"ping"}),
        json!({"op":"tree"}),
        json!({"op":"sandbox_probe"}),
        json!({"op":"claude_sandbox_probe"}),
        json!({"op":"live"}),
    ] {
        let request = IncomingRequest {
            id: "receipt".into(),
            args: RequestEnvelope::new(raw.clone()),
        };
        assert!(matches!(
            request.args.request,
            Request::Ping
                | Request::Tree(_)
                | Request::SandboxProbe
                | Request::ClaudeSandboxProbe
                | Request::Live(_)
        ));
        assert!(request.cleanup_request_id().is_none());
        assert_eq!(request.args.fingerprint(), raw.to_string());
    }
}
