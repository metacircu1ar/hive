use super::{Context, Failure, Result};
use comb::setup::{Options, Prepared};
use std::path::Path;

pub(in crate::cli) fn saved(
    root: Option<&std::path::Path>,
    socket: Option<&std::path::Path>,
    options: Options,
) -> Result<Options> {
    let default;
    let root = if let Some(root) = root {
        root
    } else {
        default =
            comb::setup::default_root().map_err(|error| Failure::remote(error, "setup", None))?;
        &default
    };
    comb::setup::transfer::load(root, socket, &options.preparation)
        .map_err(|error| Failure::remote(error, "setup", None))
}

/// Ask once, on a terminal, whether a fresh root uses local embeddings. The
/// default stays text search; --yes, dry runs, explicit choices and installer
/// apply never ask. Installer preparation asks, so its saved choice is complete.
pub(in crate::cli) fn offer_local(root: &Path, prepared: Prepared) -> comb::Result<Prepared> {
    use std::io::IsTerminal as _;
    let options = prepared.options();
    if options.yes
        || options.dry_run
        || options.local_embeddings
        || options.embedding_config.is_some()
        || options.preparation.apply_prepared.is_some()
        || !std::io::stdin().is_terminal()
        || !std::io::stderr().is_terminal()
        || root.join(".comb").try_exists()?
        || root.join(".comb-setup.json").try_exists()?
    {
        return Ok(prepared);
    }
    eprint!(
        "Comb uses text search by default. Turn on local semantic search? This downloads \
            8 GB (the Qwen3-Embedding-8B model, Apache-2.0) and runs Comb's bundled \
            llama-server. [y/N] "
    );
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(if matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
        prepared.request_local()
    } else {
        prepared
    })
}

pub(in crate::cli) fn prepare(options: Options) -> Result<Prepared> {
    options
        .validate_input()
        .map_err(|error| Failure::remote(error, "setup", None))?;
    if options.preparation.prepare_output.is_some()
        && let Some(path) = &options.embedding_config
    {
        use std::os::unix::fs::OpenOptionsExt;
        let context = Context::new("setup", "--embedding-config").file(path);
        let metadata = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .and_then(|file| file.metadata())
            .map_err(|cause| context.io(cause))?;
        if !metadata.is_file() {
            return Err(context.invalid(
                hive_input::Code::InvalidFile,
                "a reusable regular file; installer setup reads it before and after activation",
            ));
        }
    }
    let config = options
        .embedding_config
        .as_deref()
        .map(|path| Context::new("setup", "--embedding-config").read_embedding(path))
        .transpose()?;
    let path = options.embedding_config.clone();
    Prepared::from_config(options, config)
        .map_err(|error| Failure::remote(error, "setup", None).path(path.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn selected_configuration_causes_stay_private_and_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("selected");
        for malformed in [false, true] {
            if malformed {
                std::fs::write(&path, b"PRIVATE: [").unwrap();
            }
            let error = prepare(Options {
                embedding_config: Some(path.clone()),
                ..Options::default()
            })
            .err()
            .unwrap();
            let cause = error.cause().unwrap();
            if malformed {
                assert_eq!(cause.code, comb::ErrorCode::InvalidEmbeddingConfig);
                assert!(
                    cause
                        .source()
                        .unwrap()
                        .downcast_ref::<serde_yaml_ng::Error>()
                        .is_some()
                );
            } else {
                assert_eq!(
                    cause
                        .source()
                        .unwrap()
                        .downcast_ref::<std::io::Error>()
                        .unwrap()
                        .kind(),
                    std::io::ErrorKind::NotFound
                );
            }
            let output = error.render();
            assert_eq!(output.exit_code, 2);
            let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(json["error"]["field"], "--embedding-config");
            assert!(json["error"].get("file_committed").is_none());
            assert!(!output.stdout.contains("PRIVATE"));
            assert!(!output.stderr.contains("PRIVATE"));
        }
    }
}
