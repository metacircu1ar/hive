use super::{Login, Result, Values, removal};
use crate::cli::input::Context;
use crate::config::Provider;
use hive_input::validate;

impl Values {
    pub(super) fn validate_supplied(&mut self) -> Result<()> {
        self.validate_supplied_using(|value| {
            hive_setup_support::environment::service_path(Some(value))
        })
    }

    fn validate_supplied_using(
        &mut self,
        read: impl FnOnce(&str) -> std::io::Result<String>,
    ) -> Result<()> {
        self.provider_choices()?;
        for (field, value, rule) in [
            (
                "--codex-profile",
                &self.codex_profile,
                validate::config_name as fn(&str) -> _,
            ),
            (
                "--claude-profile",
                &self.claude_profile,
                validate::config_name,
            ),
            ("--codex-model", &self.codex_model, validate::model),
            ("--claude-model", &self.claude_model, validate::model),
            ("--codex-effort", &self.codex_effort, validate::effort),
            ("--claude-effort", &self.claude_effort, validate::effort),
        ] {
            if let Some(value) = value {
                rule(value).map_err(|cause: validate::Violation| {
                    Context::new("setup", field)
                        .invalid(cause.code(), "Invalid setup option.", cause.expected())
                        .with_source(cause)
                        .error
                })?;
            }
        }
        if let Some(value) = &self.service_path {
            self.service_path = Some(read(value).map_err(|cause| {
                Context::new("setup", "--service-path")
                    .selected_path(
                        cause,
                        "a colon-separated list of absolute existing directories without \
                                control characters or empty entries",
                    )
                    .error
            })?);
        }
        Ok(())
    }

    fn provider_choices(&self) -> Result<()> {
        let Some(providers) = &self.provider else {
            return Ok(());
        };
        if providers.is_empty()
            || providers
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != providers.len()
        {
            return Err(removal::invalid(
                "--provider",
                "Invalid provider selection.",
                "unique codex/claude choices, at least one",
            ));
        }
        if self
            .default_provider
            .is_some_and(|provider| !providers.contains(&provider))
        {
            return Err(removal::invalid(
                "--default-provider",
                "Default provider is not selected.",
                "a provider included in --provider",
            ));
        }
        if self.login == Some(Login::Device) && providers.contains(&Provider::Claude) {
            return Err(removal::invalid(
                "--login",
                "Device login is Codex-only.",
                "browser or skip when Claude is selected",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;
    #[test]
    fn native_path_faults_cross_the_actual_setup_admission_without_becoming_input() {
        for errno in [libc::EIO, libc::EMFILE, libc::EACCES] {
            let mut values = Values {
                service_path: Some("/bin".into()),
                ..Values::default()
            };
            let error = values
                .validate_supplied_using(|_| Err(std::io::Error::from_raw_os_error(errno)))
                .unwrap_err();
            assert_eq!(
                error.code(),
                if errno == libc::EACCES {
                    "invalid_path"
                } else {
                    "state_io"
                }
            );
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(errno)
            );
        }
        let mut values = Values {
            service_path: Some(String::new()),
            ..Values::default()
        };
        let error = values.validate_supplied().unwrap_err();
        assert_eq!(error.code(), "invalid_value");
        assert_eq!(crate::api::value(error)["field"], "--service-path");
    }
}
