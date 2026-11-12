//! Journal a stopped unit refresh independently of binary publication.
use super::{Controller, Spec, held};
use crate::{Result, files, invalid};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Refresh {
    #[serde(deserialize_with = "hive_input::decode::object")]
    old: Spec,
    #[serde(deserialize_with = "hive_input::decode::object")]
    next: Spec,
}
impl Controller<'_> {
    pub(super) fn recover_refresh(&self) -> Result<()> {
        let path = self
            .layout
            .descriptor(self.scope)?
            .with_extension("refresh.json");
        if let Some(change) = read(&path)? {
            held::restore_artifact(self, &change.old, &change.next, true)?;
            files::remove(&path, &files::encode(&change)?)?;
        }
        Ok(())
    }
    pub(super) fn refresh_unit(&self, old: &Spec, next: &Spec) -> Result<()> {
        let mut comparable = old.clone();
        comparable.unit_contents.clone_from(&next.unit_contents);
        if comparable != *next {
            return Err(invalid(
                "Owned service settings differ; uninstall before changing installation settings.",
            ));
        }
        old.verify(self.layout, self.scope)?;
        if !self.manager().available()?
            || self.manager().state(self.scope)? == super::manager::State::Loaded
        {
            return Err(invalid(
                "Owned unit needs refresh; run service stop, then service install and service \
                    start.",
            ));
        }
        let path = self
            .layout
            .descriptor(self.scope)?
            .with_extension("refresh.json");
        files::write(
            &path,
            &files::encode(&Refresh {
                old: old.clone(),
                next: next.clone(),
            })?,
            false,
        )?;
        self.recover_refresh()
    }
}

fn read(path: &std::path::Path) -> Result<Option<Refresh>> {
    files::json_with::<Refresh>(path, |value| {
        for field in ["old", "next"] {
            if let Some(spec) = value.get(field) {
                hive_input::compatibility::record::classify(spec, &crate::compatibility::SERVICE)?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
#[test]
fn nested_newer_refresh_specs_refuse_without_changing_the_journal() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("refresh.json");
    for field in ["old", "next"] {
        let value = serde_json::json!({field: {"format":2,"unknown":true}});
        files::write(&path, &serde_json::to_vec(&value).unwrap(), true).unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = read(&path).err().unwrap();
        assert!(hive_input::compatibility::record::is_newer(&error));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
