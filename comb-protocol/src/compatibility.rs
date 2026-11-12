//! Shared-file contracts owned by the Comb protocol crate.
use hive_input::compatibility::{Entry, Released};
pub const MCP: &[&str] = &["2025-11-25"];
pub const CONNECTION: Entry = Entry {
    name: "connection",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "2326c35b7442f7d784ddc3b1abfabb8275615293fac787bf859bce5eba3a33b9",
    }],
};
pub const SERVICE: Entry = Entry {
    name: "service_protocol",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "eca2cb910ba2387b1b3600c79c3da6a5b928de69a62f95d6a07c2f1f566b65cc",
    }],
};
pub const SERVICE_PROTOCOL: u32 = SERVICE.current;
pub const SERVICE_RANGE: hive_input::compatibility::ProtocolRange =
    hive_input::compatibility::ProtocolRange::for_entry(&SERVICE);
pub const FORMATS: &[Entry] = &[CONNECTION, SERVICE];

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}
