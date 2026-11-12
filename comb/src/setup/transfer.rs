//! Complete standalone setup planning before an installer's activation boundary.
use super::{Error, Options, Result};
use hive_setup_support::preparation::{self, Arguments, Product, Resources, Saved};
use std::path::Path;

pub(crate) fn save(root: &Path, socket: Option<&Path>, options: &Options) -> Result<()> {
    let args = &options.preparation;
    let resources: Resources = preparation::read(
        args.prepare_resources
            .as_deref()
            .ok_or_else(Error::internal_serialization)?,
        None,
    )?;
    resources.validate()?;
    let saved = Saved {
        format: hive_input::compatibility::Exact,
        product: Product::Comb,
        executable_sha256: preparation::executable_digest()?,
        resources,
        scope_sha256: scope(root, socket)?,
        answers: options,
    };
    preparation::write(
        args.prepare_output
            .as_deref()
            .ok_or_else(Error::internal_serialization)?,
        &saved,
    )?;
    Ok(())
}
pub fn load(root: &Path, socket: Option<&Path>, args: &Arguments) -> Result<Options> {
    let saved: Saved<Options> = preparation::read(
        args.apply_prepared
            .as_deref()
            .ok_or_else(Error::internal_serialization)?,
        args.prepared_sha256.as_deref(),
    )?;
    saved.validate(
        Product::Comb,
        &scope(root, socket)?,
        &preparation::executable_digest()?,
    )?;
    saved.resources.check_installed(Product::Comb)?;
    let mut options = saved.answers;
    options.yes = true;
    Ok(options)
}
fn scope(root: &Path, socket: Option<&Path>) -> Result<String> {
    let bytes = serde_json::to_vec(&(root, socket))
        .map_err(|cause| Error::internal_serialization().caused_by(cause))?;
    Ok(preparation::digest(&bytes))
}
