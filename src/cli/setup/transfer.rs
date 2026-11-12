//! Save actual complete choices only after the same full planner has succeeded.
use super::{
    Cli, CommandOutcome, Context, Options, Result, api, invalid, io, options::Resolved,
    prepared::PreparedSetup, resources::View,
};
use hive_setup_support::preparation::{self, Product, Resources, Saved};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct Prepared {
    prepared: bool,
}
pub(super) fn run(cli: &Cli, config: &Path, root: &Path, args: &Options) -> Result<CommandOutcome> {
    if let Some(path) = &args.preparation.prepare_output {
        let resources: Resources = preparation::read(
            args.preparation
                .prepare_resources
                .as_deref()
                .ok_or_else(|| invalid("Setup preparation resources are missing."))?,
            None,
        )
        .map_err(io)?;
        let mut resolved = args.resolve()?;
        if resolved.values.uninstall == Some(true) {
            return Err(invalid(
                "Use hive-install --uninstall for removal, not forwarded setup flags.",
            ));
        }
        let view = View::staged(&resources, &resolved.values)?;
        let prepared = PreparedSetup::inspect_resources(
            Context {
                cli,
                config_path: config,
                root,
                options: &resolved.values,
            },
            Some(view),
        )
        .map_err(|error| resolved.attribute(error))?;
        resolved.values = prepared.options.clone();
        let saved = Saved {
            format: hive_input::compatibility::Exact,
            product: Product::Hive,
            executable_sha256: preparation::executable_digest().map_err(io)?,
            resources,
            scope_sha256: scope(cli, config, root)?,
            answers: resolved.complete()?,
        };
        preparation::write(path, &saved).map_err(io)?;
        return Ok(CommandOutcome {
            success: true,
            input: None,
            value: api::value(Prepared { prepared: true }),
        });
    }
    apply(cli, config, root, args)
}
fn apply(cli: &Cli, config: &Path, root: &Path, args: &Options) -> Result<CommandOutcome> {
    let saved: Saved<Resolved> = preparation::read(
        args.preparation
            .apply_prepared
            .as_deref()
            .ok_or_else(|| invalid("Prepared setup input is missing."))?,
        args.preparation.prepared_sha256.as_deref(),
    )
    .map_err(io)?;
    saved
        .validate(
            Product::Hive,
            &scope(cli, config, root)?,
            &preparation::executable_digest().map_err(io)?,
        )
        .map_err(io)?;
    let mut resolved = saved.answers.complete()?;
    resolved.values.yes = Some(true);
    let view = View::installed(&saved.resources, &resolved.values)?;
    let context = Context {
        cli,
        config_path: config,
        root,
        options: &resolved.values,
    };
    PreparedSetup::inspect_resources(context, Some(view))?
        .run(context)
        .map_err(|error| resolved.attribute(error))
}
fn scope(cli: &Cli, config: &Path, root: &Path) -> Result<String> {
    let bytes = serde_json::to_vec(&(
        config,
        root,
        cli.codex_bin.as_encoded_bytes(),
        cli.claude_bin.as_encoded_bytes(),
        cli.probe_timeout,
        cli.login_timeout,
    ))
    .map_err(|cause| invalid("Cannot identify setup scope.").with_source(cause))?;
    Ok(preparation::digest(&bytes))
}
