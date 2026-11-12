//! Input relationships and explicit service PATHs, before managed setup or sockets.
use super::{catalog, operational};
use crate::cli::args::{Cli, CredentialCommand, RawCommand};
use comb::{search::Mode, values::Timestamp};
use hive_input::{Code, InputError, Product, Rendered};

pub(super) fn validate(cli: &Cli, path: &str) -> Result<(), Rendered> {
    service_inputs(cli, path)?;
    match &cli.command {
        RawCommand::Init { dir: Some(_) } if cli.comb.is_some() => {
            return Err(rejected(
                path,
                Code::ConflictingArguments,
                "DIR",
                "either init DIR or --root DIR init",
            ));
        }
        RawCommand::Consolidate(_) if cli.run.is_none() => {
            return Err(rejected(
                path,
                Code::MissingArgument,
                "--run",
                "an explicit run name for consolidation",
            ));
        }
        RawCommand::Search(args) => {
            if args.mode == Mode::Text && args.embedding_config.is_some() {
                return Err(rejected(
                    path,
                    Code::ConflictingArguments,
                    "--embedding-config",
                    "embedding configuration only with --mode hybrid or --mode vector",
                ));
            }
            search_range(args.after.as_deref(), args.before.as_deref(), path)?;
        }
        RawCommand::Credential {
            command: CredentialCommand::Create { reviewer_role, .. },
        } => {
            roles(reviewer_role, path)?;
        }
        RawCommand::GraphQuery(args) => args
            .validate_input()
            .map_err(crate::cli::input::Failure::render)?,
        RawCommand::Setup(options) => options
            .validate_input()
            .map_err(|error| crate::cli::input::Failure::remote(error, "setup", None).render())?,
        _ => {}
    }
    Ok(())
}

fn service_inputs(cli: &Cli, path: &str) -> Result<(), Rendered> {
    if let RawCommand::Service { command, .. } = &cli.command {
        hive_setup_support::service::admission::validate(command).map_err(|failure| {
            crate::cli::input::Context::new("service install", failure.field)
                .selected_path(failure.cause, failure.expected)
                .render()
        })?;
    }
    service_path(cli)?;
    if let RawCommand::Service { command, logging } = &cli.command
        && !matches!(
            command,
            hive_setup_support::service::Command::Install { .. }
        )
        && let Some(field) = logging.explicit()
    {
        return Err(rejected(
            path,
            Code::ConflictingArguments,
            field,
            "logging options only with service install",
        ));
    }
    Ok(())
}

fn roles(roles: &[String], path: &str) -> Result<(), Rendered> {
    comb_protocol::service::auth::validate_reviewer_roles(roles).map_err(|_| {
        rejected(
            path,
            Code::InvalidValue,
            "--reviewer-role",
            "at most 64 unique role names, each 1..512 UTF-8 bytes without control characters",
        )
    })
}

pub(super) fn rejected(path: &str, code: Code, field: &str, expected: &str) -> Rendered {
    let Some(examples) = catalog::examples(path) else {
        return operational();
    };
    let Ok(error) = InputError::new(
        code,
        "Invalid argument combination or value.",
        field,
        expected,
        examples.as_slice().to_vec(),
    ) else {
        return operational();
    };
    error
        .render(Product::Comb)
        .unwrap_or_else(|_| operational())
}

fn service_path(cli: &Cli) -> Result<(), Rendered> {
    selected_service_path(cli, |value| {
        hive_setup_support::environment::service_path(Some(value))
    })
    .map_err(crate::cli::input::Failure::render)
}

fn selected_service_path(
    cli: &Cli,
    read: impl FnOnce(&str) -> std::io::Result<String>,
) -> crate::cli::input::Result<()> {
    let (command, service_path) = match &cli.command {
        RawCommand::Setup(options) => ("setup", options.service_path.as_deref()),
        RawCommand::Service {
            command: hive_setup_support::service::Command::Install { service_path, .. },
            ..
        } => ("service install", service_path.as_deref()),
        _ => return Ok(()),
    };
    if let Some(value) = service_path {
        read(value).map_err(|cause| {
            crate::cli::input::Context::new(command, "--service-path").selected_path(
                cause,
                "a colon-separated list of absolute existing directories without control \
                characters or empty entries",
            )
        })?;
    }
    Ok(())
}

fn search_range(after: Option<&str>, before: Option<&str>, path: &str) -> Result<(), Rendered> {
    let parse = |text: Option<&str>, field| {
        text.map(str::parse::<Timestamp>)
            .transpose()
            .map_err(|_| rejected(path, Code::InvalidValue, field, "an RFC3339 timestamp"))
    };
    let after = parse(after, "--after")?;
    let before = parse(before, "--before")?;
    comb_protocol::search::date_range(after.as_ref(), before.as_ref()).map_err(|_| {
        rejected(
            path,
            Code::ConflictingArguments,
            "--after",
            "a timestamp at or before --before (compared as instants)",
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::error::Error as _;

    #[test]
    fn path_faults_cross_real_setup_and_service_admission_with_their_cause() {
        for args in [vec!["comb", "setup"], vec!["comb", "service", "install"]] {
            let cli =
                Cli::try_parse_from(args.into_iter().chain(["--service-path", "/bin"])).unwrap();
            for errno in [libc::EIO, libc::EMFILE, libc::EACCES] {
                let error =
                    selected_service_path(&cli, |_| Err(std::io::Error::from_raw_os_error(errno)))
                        .unwrap_err();
                let cause = error.operational().or_else(|| error.cause()).unwrap();
                assert_eq!(
                    cause
                        .source()
                        .unwrap()
                        .downcast_ref::<std::io::Error>()
                        .unwrap()
                        .raw_os_error(),
                    Some(errno)
                );
                let rendered = error.render();
                assert_eq!(
                    rendered.exit_code,
                    if errno == libc::EACCES { 2 } else { 1 }
                );
                if errno == libc::EACCES {
                    let json: serde_json::Value = serde_json::from_str(&rendered.stdout).unwrap();
                    assert_eq!(json["error"]["field"], "--service-path");
                }
            }
            let error = selected_service_path(&cli, |_| {
                Err(hive_setup_support::invalid("Invalid service PATH."))
            })
            .unwrap_err();
            assert_eq!(error.code(), "invalid_value");
            assert_eq!(error.render().exit_code, 2);
        }
    }
}
