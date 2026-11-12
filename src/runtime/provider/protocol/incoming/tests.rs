use super::*;

#[test]
fn owned_catalog_pages_keep_opaque_entries_and_arbitrary_cursors() {
    let entries = vec![
        json!(null),
        json!(17),
        json!(["future"]),
        json!({"unknown": [1,2]}),
    ];
    let cursor = json!({"future":"cursor"});
    let page = Response::parse(json!({"data":entries, "nextCursor":cursor, "unknown":false}));
    assert_eq!(page.cursor(), &cursor);
    assert_eq!(page.into_catalog_data(), Some(entries));
    for invalid in [
        json!(null),
        json!([]),
        json!([[]]),
        json!({}),
        json!({"data":null}),
        json!({"data":{}}),
    ] {
        assert!(Response::parse(invalid).into_catalog_data().is_none());
    }
    assert_eq!(
        Response::parse(json!({"data":[]})).into_catalog_data(),
        Some(Vec::new())
    );
}

#[test]
fn response_metadata_shares_the_frame_projection() {
    let frame = IncomingFrame::parse(json!({"id":1,"result":{
        "model":"model","data":vec![0; 4096]}}));
    let response = frame.result().expect("fixture has a result");
    assert!(Arc::ptr_eq(&frame.fields, &response.fields));
    let cloned = response.clone();
    assert!(Arc::ptr_eq(&response.fields, &cloned.fields));
    drop(frame);
    assert_eq!(response.model(), Some("model"));
    assert_eq!(response.data_is_empty(), Some(false));
    assert_eq!(
        cloned.into_raw()["data"].as_array().map(Vec::len),
        Some(4096)
    );
}

#[test]
fn frame_presence_and_malformed_siblings_keep_native_decisions() {
    let values = [
        Value::Null,
        json!(false),
        json!(1),
        json!(1.0),
        json!("future"),
        json!([]),
        json!({}),
        json!({"id":"turn","status":17}),
    ];
    for key in ["id", "method", "params", "result", "error"] {
        for value in &values {
            let mut raw = json!({"id":1,"result":{"model":"ok"},"extra":{"opaque":[null,5]}});
            raw[key] = value.clone();
            let frame = IncomingFrame::parse(raw.clone());
            assert_eq!(frame.is_response(), raw.get("method").is_none());
            assert_eq!(
                frame.is_request(),
                raw.get("method").is_some() && raw.get("id").is_some()
            );
            assert_eq!(frame.has_id(), raw.get("id").is_some());
            assert_eq!(frame.has_result(), raw.get("result").is_some());
            assert_eq!(frame.has_error(), raw.get("error").is_some());
            assert_eq!(
                frame.matches_response(1),
                raw.get("method").is_none() && raw["id"] == 1u64
            );
            assert_eq!(
                frame.result().map(Response::into_raw),
                raw.get("result").cloned()
            );
            assert_eq!(frame.raw(), &raw);
        }
    }
    for raw in [
        json!({}),
        json!({"method":null}),
        json!({"method":null,"id":null}),
        json!([]),
        json!([1, "method"]),
        json!(true),
        Value::Null,
    ] {
        let frame = IncomingFrame::parse(raw.clone());
        assert_eq!(frame.has_id(), raw.get("id").is_some());
        assert_eq!(frame.has_result(), raw.get("result").is_some());
        assert_eq!(frame.has_error(), raw.get("error").is_some());
        assert_eq!(frame.is_response(), raw.get("method").is_none());
        assert_eq!(
            frame.is_request(),
            raw.get("method").is_some() && raw.get("id").is_some()
        );
    }
}
