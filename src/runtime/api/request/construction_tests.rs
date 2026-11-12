use super::*;

#[test]
fn fresh_internal_requests_keep_the_original_raw_fingerprint_and_decode_path() {
    for request in [
        Request::Ping,
        Request::LoginRestored(super::AuthArgs {
            auth: "main".to_owned().into(),
        }),
        Request::LoginRestored(super::AuthArgs {
            auth: "quoted\"é\0".to_owned().into(),
        }),
    ] {
        let raw = crate::api::value(&request);
        let previous = RequestEnvelope::new(raw.clone());
        let typed = RequestEnvelope::from_request(request);
        assert_eq!(typed.fingerprint(), raw.to_string());
        assert_eq!(typed.fingerprint(), previous.fingerprint());
        assert_eq!(
            std::mem::discriminant(&typed.request),
            std::mem::discriminant(&previous.request)
        );
    }
}

#[test]
fn received_requests_keep_unknown_keys_and_explicit_nulls_in_their_fingerprint() {
    let raw: Value = serde_json::from_str(r#"{"op":"ping","future":null,"extra":[7]}"#).unwrap();
    let fingerprint = RequestEnvelope::new(raw.clone()).fingerprint();
    assert_eq!(fingerprint, raw.to_string());
    assert_ne!(
        fingerprint,
        RequestEnvelope::from_request(Request::Ping).fingerprint()
    );
}
