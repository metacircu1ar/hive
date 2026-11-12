//! Private installer/product maintenance protocol; one current structural format.
use crate::{
    Result, files, invalid,
    removal::{Action, Object},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

#[derive(Default, clap::Args)]
pub struct Args {
    #[arg(long)]
    pub apply: Option<PathBuf>,
    #[arg(long)]
    pub knowledge_root: Option<PathBuf>,
    #[arg(long)]
    pub bridge_file: Option<PathBuf>,
    #[arg(long)]
    pub detach_comb: bool,
    #[arg(long)]
    pub purge_comb_root: Option<PathBuf>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(deserialize_with = "Option::deserialize")]
    pub config: Option<PathBuf>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub state: Option<PathBuf>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub root: Option<PathBuf>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub bridge: Option<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan<T> {
    pub format: hive_input::compatibility::Exact<{ crate::compatibility::MAINTENANCE.current }>,
    pub product: String,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub scope: Scope,
    #[serde(deserialize_with = "hive_input::decode::objects")]
    pub retained: Vec<Object>,
    #[serde(deserialize_with = "hive_input::decode::objects")]
    pub removals: Vec<Action>,
    pub details: T,
}
impl<T: Serialize + DeserializeOwned> Plan<T> {
    pub fn read(path: &Path, product: &str) -> Result<Self> {
        crate::names::check_output_name(path)?;
        let plan: Self = files::versioned_json(path, &crate::compatibility::MAINTENANCE)?
            .ok_or_else(|| invalid("Missing purge receipt."))?;
        if plan.product != product || plan.removals.len() > 256 || plan.retained.len() > 256 {
            return Err(invalid("Unsupported purge receipt."));
        }
        for action in &plan.removals {
            action.validate()?;
        }
        Ok(plan)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_tracking(path, &mut false)
    }
    pub fn save_tracking(&self, path: &Path, changed: &mut bool) -> Result<()> {
        crate::names::check_output_name(path)?;
        files::write_tracking(path, &files::encode(self)?, true, changed)
    }
    pub fn remove(&mut self, index: usize, receipt: &Path) -> Result<()> {
        self.remove_tracking(index, receipt, &mut false)
    }
    pub fn remove_tracking(
        &mut self,
        index: usize,
        receipt: &Path,
        changed: &mut bool,
    ) -> Result<()> {
        crate::names::check_output_name(receipt)?;
        self.removals
            .get_mut(index)
            .ok_or_else(|| invalid("Invalid purge action."))?
            .remove_tracking(changed)?;
        self.save_tracking(receipt, changed)
    }
    pub fn check_retained(&self) -> Result<()> {
        for object in &self.retained {
            object.check()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserved_receipt_fails_before_read_save_or_removal() {
        for name in [".hive-tmp-state-ABC123", ".comb-tmp-admin-ABC123"] {
            let temp = tempfile::tempdir().unwrap();
            let object = temp.path().join("owned");
            files::write(&object, b"keep", false).unwrap();
            let mut plan = Plan {
                format: hive_input::compatibility::Exact,
                product: "hive".into(),
                scope: Scope::default(),
                retained: vec![],
                removals: vec![Action::capture(&object, "test").unwrap()],
                details: (),
            };
            let receipt = temp.path().join(name);
            assert!(plan.save(&receipt).is_err());
            assert!(!receipt.exists());
            files::write(&receipt, &files::encode(&plan).unwrap(), false).unwrap();
            let before = std::fs::read(&receipt).unwrap();
            assert!(Plan::<()>::read(&receipt, "hive").is_err());
            assert!(plan.remove(0, &receipt).is_err());
            assert_eq!(std::fs::read(&object).unwrap(), b"keep");
            assert_eq!(std::fs::read(&receipt).unwrap(), before);
            assert!(!plan.removals[0].done);
            assert!(!plan.removals[0].quarantine.exists());
        }
    }
}
