//! Comb's root and derived-data contracts frozen for the first supported release.
use hive_input::compatibility::{Entry, Released};
pub const ROOT: Entry = Entry {
    name: "root",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "ad669ffb02cc28b9843648ad13fe5d1da938c4fae87d02ad3db7e6282ab93449",
    }],
};
pub const SEAL: Entry = Entry {
    name: "seal",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "e676841a8a9b20657e54db045fabed6dd9cc3bdade5bacfee8218bc70dd7c9c4",
    }],
};
pub const INDEX: Entry = Entry {
    name: "index",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "c2b6d0065bd3dcb7d441cdfca03dae62708fba53d46410e05f0dc870f68a0151",
    }],
};
pub const VECTORS: Entry = Entry {
    name: "vectors",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "134da166146e773dec65118bb7615a7ec3807c74ad072abae330719f21b6f878",
    }],
};
pub const FORMATS: &[Entry] = &[ROOT, SEAL, INDEX, VECTORS];

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}
