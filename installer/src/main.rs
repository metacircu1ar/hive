#![expect(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact reviewed maintainer write inventory."
)]
//! Native offline installer; the POSIX bootstrap handles transport.
mod archive;
mod cancellation;
mod compatibility;
mod diagnostics;
mod error;
pub(crate) use error::Error;
mod lifecycle;
mod manifest;
mod native;
mod options;
mod ownership;
mod platform;
mod publication;
mod publish;
mod purge;
mod services;
mod setup;
mod uninstall;

use options::Args;
use std::os::unix::fs::PermissionsExt;
type Result<T> = std::result::Result<T, crate::Error>;

fn execute(args: &Args) -> Result<u8> {
    if args.uninstall {
        return remove(args).map(|()| 0);
    }
    let target = manifest::native_target()?;
    let prefix = args.prefix()?;
    let source = args
        .staged_from
        .as_ref()
        .or(args.source.as_ref())
        .map(std::path::PathBuf::as_path)
        .ok_or("--from is required for installation")?;
    let staged = publication::Owned::new(
        tempfile::Builder::new()
            .prefix("hive-install-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?,
    )?;
    let result = install_parts(args, &prefix, source, &target, staged.path());
    staged.finish(result)
}
fn install_parts(
    args: &Args,
    prefix: &std::path::Path,
    source: &std::path::Path,
    target: &str,
    staging: &std::path::Path,
) -> Result<u8> {
    let selected = platform::selected(args)?;
    refuse_kept_server(
        args,
        prefix,
        target,
        args.no_comb_llama || selected.skipped_llama,
    )?;
    let manifests = selected
        .parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let manifest = archive::unpack(source, staging, part, target)
                .map_err(|cause| manifest::asset(format!("{part}-{target}.tar.gz"), cause))?;
            tracing::info!(name: "install.archive_unpacked", count = index + 1, success = true);
            Ok(manifest)
        })
        .collect::<Result<Vec<_>>>()?;
    let setup = setup::Preparation::new(prefix, staging, &manifests, args)?;
    if let setup::Preparation::Exit(code) = &setup {
        return Ok(*code);
    }
    let installed = publish::install_with(
        prefix,
        staging,
        &manifests,
        &mut services::Services::new(args.yes)?,
        |_| Ok(()),
    )?;
    println!(
        "Installed {} into {}",
        parts_description(&manifests),
        prefix.display()
    );
    if let Some(installed) = installed {
        println!(
            concat!(
                "Retained {} generation directories ",
                "({} bytes in distinct file inodes in image storage)."
            ),
            installed.generations, installed.bytes
        );
    } else {
        println!(
            "Installation completed, but retained-image usage could not be measured. \
                Inspect the installation log."
        );
    }
    next_steps(prefix);
    setup.apply(prefix)
}

fn next_steps(prefix: &std::path::Path) {
    println!("Old generations are retained; automatic pruning is not enabled.");
    println!(
        "Owned services were restored to their previous running state. Manual \
            daemons require a manual restart."
    );
    println!(
        "Add {}/bin to PATH. State, logins and configuration were not modified.",
        prefix.display()
    );
}

/// `--no-comb-llama` only skips adding the bundled server. An installed one would
/// stay beside the new Comb, which keeps running it, so refuse before any archive
/// or setup work. This reads without the installation lock: a concurrent
/// installation that adds the server afterwards only keeps it, as before.
fn refuse_kept_server(
    args: &Args,
    prefix: &std::path::Path,
    target: &str,
    skipping: bool,
) -> Result<()> {
    if !skipping {
        return Ok(());
    }
    let manager = prefix.join("lib/hive");
    let Some(pointer) = publish::current(&manager)? else {
        return Ok(());
    };
    if !publish::load(&manager, Some(pointer.as_path()), target)?
        .parts
        .contains_key(manifest::LLAMA_COMPONENT)
    {
        return Ok(());
    }
    if !args.no_comb_llama {
        return Err(
            "The installed bundled server requires macOS 13.3 or newer. \
            Uninstall Comb first (--uninstall --standalone-comb), then reinstall \
            it without the unsupported optional server."
                .into(),
        );
    }
    Err(format!(
        "--no-comb-llama does not remove the bundled comb-llama-server installed in {}. \
            Omit --no-comb-llama to update it with Comb, or uninstall Comb first \
            (--uninstall --standalone-comb) and then install it again with --no-comb-llama.",
        prefix.display()
    )
    .into())
}

fn remove(args: &Args) -> Result<()> {
    if args.purge {
        return purge::run(args);
    }
    let components = args.components();
    let removed = uninstall::run(
        &args.prefix()?,
        &components,
        uninstall::Options {
            yes: args.yes,
            dry_run: args.dry_run,
        },
    )?;
    println!(
        "{}: {} active components remain. Retained maintenance images: {:?}",
        if args.dry_run {
            "Uninstall preview"
        } else {
            "Uninstalled selected components"
        },
        removed.active,
        removed.binaries
    );
    Ok(())
}

fn parts_description(parts: &[manifest::Manifest]) -> String {
    parts
        .iter()
        .map(|p| p.part.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
fn main() -> std::process::ExitCode {
    let args = match Args::read() {
        Ok(args) => args,
        Err(output) => return std::process::ExitCode::from(options::parsing::emit(&output)),
    };
    match diagnostics::standalone(|| execute(&args)) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("hive install: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
