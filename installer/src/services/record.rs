//! Admit the installation and nested service versions before exact journal decoding.
use crate::{Result, lifecycle::Change};
use hive_setup_support::{files, service::Spec};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Saved {
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub(super) spec: Spec,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub(super) next: Spec,
    pub(super) loaded: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub(super) format: crate::compatibility::Format,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub(super) change: Change,
    #[serde(deserialize_with = "hive_input::decode::objects")]
    pub(super) services: Vec<Saved>,
}

pub(super) fn read(path: &Path) -> Result<Option<Journal>> {
    Ok(files::json_with(path, |value| {
        use hive_input::compatibility::record::classify;
        classify(value, &crate::compatibility::INSTALLATION)?;
        if let Some(services) = value.get("services").and_then(serde_json::Value::as_array) {
            for service in services {
                for field in ["spec", "next"] {
                    if let Some(spec) = service.get(field) {
                        classify(spec, &hive_setup_support::compatibility::SERVICE)?;
                    }
                }
            }
        }
        Ok(())
    })?)
}

#[cfg(test)]
#[test]
fn nested_newer_install_service_specs_preserve_the_pending_journal() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("pending-install.json");
    for field in ["spec", "next"] {
        let value =
            serde_json::json!({"format":1,"services":[{field:{"format":2,"unknown":true}}]});
        files::write(&path, &serde_json::to_vec(&value).unwrap(), true).unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = read(&path).err().unwrap();
        assert!(hive_input::compatibility::record::is_newer(error.as_ref()));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
