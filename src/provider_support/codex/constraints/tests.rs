use super::*;

#[test]
fn sequence_type_names_keep_borrowed_text_and_ignored_nontext_slots() {
    let value = serde_json::json!(["object", null, 3, {}, ["string"], false, "null"]);
    let kind = Type::deserialize(&value).expect("the advertised type sequence is accepted");
    assert!(!kind.literal("object"));
    assert!(kind.contains("object"));
    assert!(kind.contains("null"));
    assert!(!kind.contains("string"));
    let Type::Names(names) = kind else {
        panic!("sequence type names must remain a sequence");
    };
    assert_eq!(names.len(), 7);
    assert!(matches!(names[0].0.as_ref(), Some(Cow::Borrowed("object"))));
    assert!(names[1..6].iter().all(|name| name.0.is_none()));
    assert!(matches!(names[6].0.as_ref(), Some(Cow::Borrowed("null"))));
}
