//! Resolve and fully plan setup before publishing an installation generation.
mod invocation;
#[cfg(test)]
mod tests;
use crate::{Result, manifest::Manifest, options::Args};
use hive_setup_support::preparation::{self, Product, Resources, Saved};
use invocation::Invocation;
use std::path::{Path, PathBuf};

pub(crate) enum Preparation {
    Skip,
    Exit(u8),
    Ready(Box<Prepared>),
}
pub(crate) struct Prepared {
    invocation: Invocation,
    product: Product,
    file: PathBuf,
    sha256: String,
}
impl Preparation {
    pub(crate) fn new(
        prefix: &Path,
        staging: &Path,
        manifests: &[Manifest],
        args: &Args,
    ) -> Result<Self> {
        if args.no_setup {
            return Ok(Self::Skip);
        }
        let invocation = Invocation::new(args);
        let product = if args.standalone_comb {
            Product::Comb
        } else {
            Product::Hive
        };
        let resources = resources(prefix, staging, manifests)?;
        let root = match product {
            Product::Hive => resources.hive_source.as_ref(),
            Product::Comb => resources.comb_source.as_ref(),
        }
        .ok_or("Requested setup product has no verified source")?;
        let program = root.join("bin").join(name(product));
        let resource_file = staging.join("setup-resources.json");
        preparation::write(&resource_file, &resources)?;
        let file = staging.join("setup-answers.json");
        let code = invocation
            .prepare(&program, &resource_file, &file)
            .map_err(|cause| retain(staging, cause))?;
        if !matches!(code, 0 | 2) {
            return Err(retain(
                staging,
                format!("Setup preparation failed (exit {code}); installation is unchanged.")
                    .into(),
            ));
        }
        if code != 0 || invocation.help() {
            return Ok(Self::Exit(code));
        }
        let bytes = preparation::read_private(&file)?;
        let sha256 = preparation::digest(&bytes);
        let saved: Saved<serde::de::IgnoredAny> = preparation::read(&file, Some(&sha256))?;
        let expected = manifests
            .iter()
            .find_map(|manifest| manifest.files.get(&format!("bin/{}", name(product))))
            .ok_or("Verified setup executable is missing from the manifest")?;
        if saved.product != product
            || saved.resources != resources
            || saved.executable_sha256 != expected.sha256
            || crate::manifest::hash(&program)? != expected.sha256
        {
            return Err("Setup preparation belongs to another verified source".into());
        }
        Ok(Self::Ready(Box::new(Prepared {
            invocation,
            product,
            file,
            sha256,
        })))
    }
    pub(crate) fn apply(self, prefix: &Path) -> Result<u8> {
        match self {
            Self::Skip => Ok(0),
            Self::Exit(code) => Ok(code),
            Self::Ready(prepared) => {
                let program = prefix.join("bin").join(name(prepared.product));
                let result = prepared
                    .invocation
                    .apply(&program, &prepared.file, &prepared.sha256);
                let staging = prepared.file.parent().unwrap_or(prefix);
                let code = result.map_err(|cause| retain(staging, cause))?;
                if !matches!(code, 0 | 2) {
                    return Err(retain(
                        staging,
                        format!("Binaries are installed, but setup failed (exit {code}).").into(),
                    ));
                }
                Ok(code)
            }
        }
    }
}
#[derive(Debug)]
pub(crate) struct RetainedFailure {
    path: PathBuf,
    cause: crate::Error,
}
impl std::fmt::Display for RetainedFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "{}; setup evidence retained at {}",
            self.cause,
            self.path.display()
        )
    }
}
impl std::error::Error for RetainedFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
fn retain(path: &Path, cause: crate::Error) -> crate::Error {
    RetainedFailure {
        path: path.to_owned(),
        cause,
    }
    .into()
}
fn name(product: Product) -> &'static str {
    match product {
        Product::Hive => "hivectl",
        Product::Comb => "comb",
    }
}
fn resources(prefix: &Path, staging: &Path, manifests: &[Manifest]) -> Result<Resources> {
    let mut resources = Resources {
        format: hive_input::compatibility::Exact,
        prefix: prefix.to_owned(),
        hive_source: None,
        comb_source: None,
        llama_source: None,
    };
    for manifest in manifests {
        let path = staging.join(manifest.part.as_str());
        match manifest.component()? {
            "hive" => resources.hive_source = Some(path),
            "comb" => resources.comb_source = Some(path),
            crate::manifest::LLAMA_COMPONENT => resources.llama_source = Some(path),
            _ => return Err("Unknown setup component".into()),
        }
    }
    resources.validate()?;
    Ok(resources)
}
