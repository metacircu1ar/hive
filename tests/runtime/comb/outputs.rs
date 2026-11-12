//! Native catalogue replies can exceed the daemon's 16 KiB inline event limit.
use super::*;

pub(super) fn data(f: &Harness, event: &Value) -> Value {
    let Some(artifact) = event["artifact"].as_object() else {
        return event.clone();
    };
    let bytes = artifact["bytes"].as_u64().unwrap();
    assert!(bytes <= 1024 * 1024, "fixture output exceeded its bound");
    let mut text = String::new();
    let mut offset = 0;
    loop {
        let page = f.ok(json!({"op":"output","id":artifact["id"],"offset":offset,"limit":65536}));
        text.push_str(page["text"].as_str().unwrap());
        let next = page["next_offset"].as_u64().unwrap();
        assert!(next > offset && next <= bytes);
        assert_eq!(u64::try_from(text.len()).unwrap(), next);
        offset = next;
        if page["eof"].as_bool().unwrap() {
            assert_eq!(offset, bytes);
            break;
        }
    }
    serde_json::from_str(&text).unwrap()
}
