//! Read staged bundle bytes while keeping configured paths in the future install.
use super::{Result, invalid, io, options::Values, parts};
use hive_setup_support::{files, preparation::Resources};
use std::path::{Path, PathBuf};

pub(super) struct View {
    pub skills_source: PathBuf,
    pub skills_target: PathBuf,
    comb_override: Option<PathBuf>,
}
impl View {
    pub fn skill(&self, file: &str) -> Result<PathBuf> {
        let target = self.skills_target.join(file);
        if !self.skills_source.join(file).is_file() {
            return Err(invalid(format!(
                "Missing bundled skill: {}",
                target.display()
            )));
        }
        Ok(target)
    }
    pub fn normal(options: &Values) -> Result<Self> {
        let skills = parts::skills(options.skills_dir.as_deref())?;
        Ok(Self {
            skills_source: skills.clone(),
            skills_target: skills,
            comb_override: None,
        })
    }
    pub fn staged(resources: &Resources, options: &Values) -> Result<Self> {
        resources.validate().map_err(io)?;
        let source = resources
            .hive_source
            .as_ref()
            .ok_or_else(|| invalid("Hive setup preparation has no verified Hive source."))?;
        let (skills_source, skills_target) = match &options.skills_dir {
            Some(path) => (parts::skills(Some(path))?, parts::skills(Some(path))?),
            None => (
                source.join("share/hive/skills"),
                resources.prefix.join("share/hive/skills"),
            ),
        };
        let comb_override = match &resources.comb_source {
            Some(source) => Some(source.join("bin/comb")),
            None => installed_comb(&resources.prefix)?,
        };
        Ok(Self {
            skills_source,
            skills_target,
            comb_override,
        })
    }
    pub fn installed(resources: &Resources, options: &Values) -> Result<Self> {
        resources.validate().map_err(io)?;
        let prefix = parts::invoked()?
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| invalid("Installed setup has no stable prefix."))?
            .to_owned();
        if prefix != resources.prefix {
            return Err(invalid(
                "Prepared setup belongs to another installation prefix.",
            ));
        }
        let skills = match &options.skills_dir {
            Some(path) => parts::skills(Some(path))?,
            None => prefix.join("share/hive/skills"),
        };
        Ok(Self {
            skills_source: skills.clone(),
            skills_target: skills,
            comb_override: installed_comb(&prefix)?,
        })
    }
    pub fn comb(&self, explicit: Option<&Path>) -> Result<Option<PathBuf>> {
        if explicit.is_some() {
            return parts::comb(explicit);
        }
        if let Some(path) = &self.comb_override {
            return files::executable(path).map(Some).map_err(io);
        }
        parts::comb(None)
    }
}
fn installed_comb(prefix: &Path) -> Result<Option<PathBuf>> {
    let path = prefix.join("bin/comb");
    files::optional_executable(&path).map_err(io)
}
