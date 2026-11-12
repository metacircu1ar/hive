//! Shared setup records outlive the creating executable.
use hive_input::compatibility::{Entry, Released};
pub const MAINTENANCE: Entry = Entry {
    name: "maintenance",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "bda95a121750a9d7fc18f1c7ce468bf4fc476e191632d1030e213e7b0c6d9ce9",
    }],
};
pub const SERVICE: Entry = Entry {
    name: "service",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "84fffaf7daf48c3d4002e1cf5fe1649db05ee60af8cae07e505425256414a216",
    }],
};
pub const FORMATS: &[Entry] = &[MAINTENANCE, SERVICE];

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}

#[cfg(test)]
#[test]
fn compatibility_shared_record_contracts_require_current_markers() {
    fn check<T: serde::de::DeserializeOwned + serde::Serialize>(bytes: &str) {
        let value: serde_json::Value = serde_json::from_str(bytes).unwrap();
        let native: T = serde_json::from_str(bytes).unwrap();
        assert_eq!(serde_json::to_value(native).unwrap(), value);
        for field in value.as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<T>(missing).is_err(),
                "missing {field}"
            );
        }
        for marker in [
            None,
            Some(serde_json::json!(2)),
            Some(serde_json::Value::Null),
        ] {
            let mut invalid = value.clone();
            invalid.as_object_mut().unwrap().remove("format");
            if let Some(marker) = marker {
                invalid["format"] = marker;
            }
            assert!(serde_json::from_value::<T>(invalid).is_err());
        }
    }
    check::<crate::maintenance::Plan<serde_json::Value>>(include_str!(
        "compatibility/maintenance.json"
    ));
    check::<crate::service::Spec>(include_str!("compatibility/service.json"));
}
