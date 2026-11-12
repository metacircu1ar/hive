use super::*;
use serde_json::{Value, json};

#[derive(Default, Deserialize)]
struct Projection<'a> {
    #[serde(default, borrow, deserialize_with = "text")]
    text: Option<Cow<'a, str>>,
    #[serde(default, deserialize_with = "optional")]
    sibling: Option<bool>,
}

#[test]
fn text_borrows_values_and_owns_only_escaped_decoder_text() {
    let raw = json!({"text":"escaped\ntext","sibling":true});
    let projection: Projection<'_> = defaulting(&raw).unwrap();
    assert!(matches!(
        projection.text,
        Some(Cow::Borrowed("escaped\ntext"))
    ));
    let plain: Projection<'_> = serde_json::from_str(r#"{"text":"plain"}"#).unwrap();
    assert!(matches!(plain.text, Some(Cow::Borrowed("plain"))));
    let escaped: Projection<'_> = serde_json::from_str(r#"{"text":"escaped\ntext"}"#).unwrap();
    assert!(matches!(escaped.text, Some(Cow::Owned(ref s)) if s == "escaped\ntext"));
    for text in [Value::Null, json!(42), json!(false), json!([]), json!({})] {
        let raw = json!({"text":text,"sibling":true});
        let projection: Projection<'_> = defaulting(&raw).unwrap();
        assert!(projection.text.is_none());
        assert_eq!(projection.sibling, Some(true));
    }
}
