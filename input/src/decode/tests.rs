use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Record {
    count: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Nested {
    #[serde(deserialize_with = "object")]
    record: Record,
    #[serde(deserialize_with = "optional_object")]
    nullable: Option<Record>,
    #[serde(deserialize_with = "objects")]
    records: Vec<Record>,
    #[serde(deserialize_with = "optional_objects_array")]
    slots: [Option<Record>; 2],
}
#[test]
fn named_records_keep_arrays_only_where_the_format_declares_them() {
    let valid = r#"{"record":{"count":1},"nullable":null,"records":[{"count":2}],"slots":[null,{"count":3}]}"#;
    let record: Nested = from_str(valid).unwrap();
    assert_eq!(record.record.count, 1);
    assert!(record.nullable.is_none());
    assert_eq!(record.records, [Record { count: 2 }]);
    assert_eq!(record.slots[1].as_ref().unwrap().count, 3);
    for invalid in [
        valid.replace("{\"count\":1}", "[1]"),
        valid.replace("{\"count\":2}", "[2]"),
        valid.replace("{\"count\":3}", "[3]"),
        valid.replace("\"nullable\":null,", ""),
    ] {
        assert!(from_str::<Nested>(&invalid).is_err());
    }
}
#[derive(Deserialize)]
struct Maps {
    #[serde(deserialize_with = "map")]
    ids: BTreeMap<uuid::Uuid, u32>,
}
#[test]
fn maps_reject_decoded_identity_collisions() {
    let text = r#"{"ids":{"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa":1,"AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA":2}}"#;
    assert!(from_str::<Maps>(text).is_err());
    assert_eq!(from_str::<Maps>(r#"{"ids":{}}"#).unwrap().ids.len(), 0);
}
#[derive(Deserialize)]
struct Json {
    #[serde(deserialize_with = "value")]
    value: serde_json::Value,
}
#[test]
fn recursive_values_reject_duplicates_but_retain_arrays_scalars_and_text() {
    assert!(from_str::<Json>(r#"{"value":[{"x":1,"x":2}]}"#).is_err());
    let value = from_str::<Json>(r#"{"value":[null,true,3.5,"{\"x\":1,\"x\":2}"]}"#).unwrap();
    assert_eq!(value.value.as_array().unwrap().len(), 4);
}
