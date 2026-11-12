use super::*;
use serde::Deserialize;
const ENTRY: Entry = Entry::draft("fixture");
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    format: super::super::Exact<1>,
    value: String,
}
#[test]
fn newer_records_are_classified_before_unknown_or_missing_current_fields() {
    let valid: Record = decode(br#"{"format":1,"value":"current"}"#, &ENTRY).unwrap();
    assert_eq!(valid.value, "current");
    assert_eq!(valid.format, super::super::Exact::<1>);
    let error = decode::<Record>(br#"{"format":2,"new_field":true}"#, &ENTRY).unwrap_err();
    assert!(error.newer());
    let wrapped = io::Error::new(io::ErrorKind::InvalidInput, error);
    assert!(is_newer(&wrapped));
    let wrapped = io::Error::other(wrapped);
    assert!(is_newer(&wrapped));
    for bytes in [
        br#"{"format":1,"format":2,"new_field":true}"#.as_slice(),
        br#"{"format":2,"nested":{"x":1,"x":2}}"#,
        br#"{"format":1,"new_field":true}"#,
        br#"{"format":null,"value":"current"}"#,
        br#"[1,"current"]"#,
    ] {
        assert!(matches!(
            decode::<Record>(bytes, &ENTRY),
            Err(Error::Malformed(_))
        ));
    }
}
