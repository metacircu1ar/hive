//! Selected service inputs are observed before manager discovery in both products.
use super::Command;
use crate::{environment, files};
use std::io;

#[derive(Debug)]
pub struct PathFailure {
    pub field: &'static str,
    pub expected: &'static str,
    pub cause: io::Error,
}

pub fn input_code(field: &str, cause: &io::Error) -> Option<hive_input::Code> {
    if !is_input(cause) {
        return None;
    }
    Some(
        if field == "--service-path"
            && cause.kind() == io::ErrorKind::InvalidInput
            && cause.raw_os_error().is_none()
        {
            hive_input::Code::InvalidValue
        } else {
            hive_input::Code::InvalidPath
        },
    )
}

fn is_input(cause: &io::Error) -> bool {
    matches!(
        cause.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::NotADirectory
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::InvalidInput
            | io::ErrorKind::IsADirectory
    ) || matches!(
        cause.raw_os_error(),
        Some(libc::ELOOP | libc::ENAMETOOLONG | libc::ENXIO | libc::ENODEV | libc::EOPNOTSUPP)
    )
}

fn read<T>(
    field: &'static str,
    expected: &'static str,
    result: io::Result<T>,
) -> Result<T, PathFailure> {
    result.map_err(|cause| PathFailure {
        field,
        expected,
        cause,
    })
}

pub fn validate(command: &Command) -> Result<(), PathFailure> {
    let Command::Install {
        program,
        environment_file,
        service_path,
    } = command
    else {
        return Ok(());
    };
    if let Some(program) = program {
        read(
            "--program",
            "an existing executable with a UTF-8 absolute path",
            files::executable(program).and_then(|resolved| super::path_text(&resolved)),
        )?;
    }
    if let Some(path) = environment_file {
        read(
            "--environment-file",
            "an owned readable environment file with KEY=literal-value lines",
            files::absolute(path).and_then(|resolved| {
                super::path_text(&resolved)?;
                environment::read(Some(&resolved))
            }),
        )?;
    }
    if let Some(value) = service_path {
        read(
            "--service-path",
            "absolute existing directories without controls or empty entries",
            environment::service_path(Some(value)),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_service_path_is_a_value_error_and_native_errno_is_not_lexical() {
        let lexical = crate::invalid("Invalid service PATH.");
        assert_eq!(
            input_code("--service-path", &lexical),
            Some(hive_input::Code::InvalidValue)
        );
        assert_eq!(
            input_code("--program", &lexical),
            Some(hive_input::Code::InvalidPath)
        );
        for errno in [libc::EINVAL, libc::ENOENT, libc::EACCES] {
            assert_eq!(
                input_code("--service-path", &io::Error::from_raw_os_error(errno)),
                Some(hive_input::Code::InvalidPath)
            );
        }
        for errno in [libc::EIO, libc::EMFILE] {
            assert_eq!(
                input_code("--service-path", &io::Error::from_raw_os_error(errno)),
                None
            );
        }
    }
}
