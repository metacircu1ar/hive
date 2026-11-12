//! Private per-invocation setup answers, never authority to overwrite changed state.
mod storage;
#[cfg(test)]
mod tests;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use storage::{digest, executable_digest, read, read_private, write};

#[derive(Clone, Default, clap::Args)]
pub struct Arguments {
    #[arg(
        long,
        hide = true,
        conflicts_with = "apply_prepared",
        requires = "prepare_resources"
    )]
    pub prepare_output: Option<PathBuf>,
    #[arg(
        long,
        hide = true,
        conflicts_with = "prepare_output",
        requires = "prepared_sha256"
    )]
    pub apply_prepared: Option<PathBuf>,
    #[arg(long, hide = true, requires = "apply_prepared")]
    pub prepared_sha256: Option<String>,
    #[arg(long, hide = true, requires = "prepare_output")]
    pub prepare_resources: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Product {
    Hive,
    Comb,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub format: hive_input::compatibility::Exact<1>,
    pub prefix: PathBuf,
    #[serde(deserialize_with = "Option::deserialize")]
    pub hive_source: Option<PathBuf>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub comb_source: Option<PathBuf>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub llama_source: Option<PathBuf>,
}
impl Resources {
    pub fn check_installed(&self, product: Product) -> crate::Result<()> {
        self.validate()?;
        let argument = std::env::args_os()
            .next()
            .ok_or_else(|| crate::invalid("Installed setup has no invoked executable."))?;
        let program = crate::files::executable(std::path::Path::new(&argument))?;
        let name = match product {
            Product::Hive => "hivectl",
            Product::Comb => "comb",
        };
        if program != self.prefix.join("bin").join(name) {
            return Err(crate::invalid(
                "Prepared setup belongs to another installed executable.",
            ));
        }
        Ok(())
    }
    pub fn validate(&self) -> crate::Result<()> {
        if !self.prefix.is_absolute()
            || crate::files::absolute(&self.prefix)? != self.prefix
            || self.hive_source.is_none() && self.comb_source.is_none()
            || [&self.hive_source, &self.comb_source, &self.llama_source]
                .into_iter()
                .flatten()
                .any(|path| !path.is_absolute())
        {
            return Err(crate::invalid("Invalid setup resource scope."));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "T: Deserialize<'de>"))]
pub struct Saved<T> {
    pub format: hive_input::compatibility::Exact<1>,
    pub product: Product,
    pub executable_sha256: String,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub resources: Resources,
    pub scope_sha256: String,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub answers: T,
}
impl<T> Saved<T> {
    pub fn validate(&self, product: Product, scope: &str, executable: &str) -> crate::Result<()> {
        self.resources.validate()?;
        if self.product != product
            || self.scope_sha256 != scope
            || self.executable_sha256 != executable
        {
            return Err(crate::invalid(
                "Prepared setup belongs to another build or scope.",
            ));
        }
        Ok(())
    }
}
