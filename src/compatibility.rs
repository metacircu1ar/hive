//! Hive-owned data families frozen for the first supported release.
use hive_input::compatibility::{Entry, Released};

pub const EXTERNAL_MCP: &[&str] = &["2025-11-25"];
pub const CLAUDE_MCP: &[&str] = &["2024-11-05"];

pub const STATE: Entry = Entry {
    name: "state",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "9c501bef9c7f7096a6c51f47d073527fd97d04b6d1641a4d727425a470390bf1",
    }],
};
pub const SHELL: Entry = Entry {
    name: "shell_protocol",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "b7f35d9daff4bed6b351e83d1ebd9213bbe7594aa04b2591beb2216f37539bc7",
    }],
};
pub const CONTROL: Entry = Entry {
    name: "control_protocol",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "75c79c51d74c978786e023be9e27a224f456739b40abf806ce246ef1e95f6bb9",
    }],
};
pub type AgentCatalog = hive_input::compatibility::Exact<{ AGENT_CATALOG.current }>;
pub const AGENT_CATALOG: Entry = Entry {
    name: "agent_catalog",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "e72f7d92f643190d1d3f021876bb94f8903bf575e1af0924c13a7d178fb2ca14",
    }],
};
pub const CONTROL_PROTOCOL: u32 = CONTROL.current;
pub const CONTROL_RANGE: hive_input::compatibility::ProtocolRange =
    hive_input::compatibility::ProtocolRange::for_entry(&CONTROL);
pub const COLLECTOR: Entry = Entry {
    name: "collector",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "bfaa8c1d7ddf988855254f748d66bd424f05528a4a7cc5cace04fd0f341dafd0",
    }],
};
pub const CONTROLLER_EXPORT: Entry = Entry {
    name: "controller_export",
    current: 1,
    minimum: 1,
    maximum: 1,
    released: &[Released {
        product_version: "0.1.0",
        number: 1,
        fingerprint: "be049fa30c3c9d4f1da820300c53edf260be6f08bcc7ca3639ce7953b75f28a3",
    }],
};
pub const FORMATS: &[Entry] = &[
    STATE,
    COLLECTOR,
    CONTROLLER_EXPORT,
    CONTROL,
    AGENT_CATALOG,
    SHELL,
];

#[cfg(test)]
pub(crate) fn contract(name: &str, actual: &impl serde::Serialize) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/compatibility/contracts")
        .join(name);
    let mut bytes = serde_json::to_vec_pretty(actual).unwrap();
    bytes.push(b'\n');
    if std::env::var_os("COMB_RECORD_GOLDENS").is_some() {
        atomic_fs::Publisher::new(
            (hive_setup_support::names::PUBLICATION_OPTIONS).with_mode(0o644),
        )
        .write(&path, &bytes)
        .unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
}

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}

pub(crate) fn newer() -> crate::error::Error {
    crate::error::Error::newer_format(
        "This data uses a newer format. The required product release is unknown to \
            this build; keep the data unchanged and use its producing build or a newer \
            supported build.",
    )
}
