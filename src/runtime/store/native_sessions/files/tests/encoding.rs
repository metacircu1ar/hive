use super::super::*;

#[test]
fn native_file_record_retains_sorted_identity_and_path_bytes() {
    let file = File {
        path: "/native/é\"".into(),
        identity: Identity {
            device: 4,
            inode: 5,
            bytes: 3,
            modified_seconds: -1,
            modified_nanos: 2,
        },
    };
    assert_eq!(
        crate::api::json_text(&file),
        crate::api::value(&file).to_string()
    );
    assert_eq!(
        crate::api::json_text(&file),
        concat!(
            "{\"identity\":{\"bytes\":3,\"device\":4,\"inode\":5,\"modified_nanos\":2,",
            "\"modified_seconds\":-1},\"path\":\"/native/é\\\"\"}"
        )
    );
}
