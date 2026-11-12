use super::*;

#[test]
fn codex_frame_measurement_equals_the_previous_canonical_encoding_at_size_edges() {
    for text in [
        String::new(),
        "é\n\"\0".into(),
        "x".repeat(65_535),
        "x".repeat(65_536),
        "x".repeat(65_537),
    ] {
        let request = outgoing::turn_start("thread", "model", "low", Some("summary"), &text);
        let previous = crate::api::value(outgoing::request(u64::MAX, request.clone())).to_string();
        assert_eq!(frame_bytes(&request, true).unwrap(), previous.len());
    }
}
