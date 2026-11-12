//! Prove the selected method before native thread work, without persisting login replies.
use super::{Error, Result, Rpc, outgoing};
use crate::{config::Method, credentials::Credential};
use serde_json::Value;
use std::path::Path;

impl Rpc {
    pub(crate) fn initialize_authenticated(
        &mut self,
        credential: &Credential,
        home: &Path,
    ) -> Result<()> {
        self.expected_method = credential.method();
        self.private_bootstrap = self.expected_method == Method::ApiKey;
        let result = (|| {
            self.initialize()?;
            if self.claude.is_none() && self.expected_method == Method::ApiKey {
                let requirements = self.call(outgoing::config_requirements())?.into_raw();
                let config = self.call(outgoing::config_read())?.into_raw();
                if !safe_requirements(&requirements) || !safe_config(&config) {
                    return Err(Error::policy_conflict(
                        "Codex must prove ephemeral storage and the built-in OpenAI provider \
                            before API login.",
                    ));
                }
                ephemeral_home(home)?;
                let response = self
                    .call(outgoing::OutgoingRequest::ApiLogin(credential.login()?))?
                    .into_raw();
                if response.get("type").and_then(Value::as_str) != Some("apiKey") {
                    return Err(Error::authentication_required(
                        "Codex did not acknowledge the selected API authentication method.",
                    ));
                }
            }
            self.validate_account()?;
            if self.claude.is_none() && self.expected_method == Method::ApiKey {
                ephemeral_home(home)?;
            }
            Ok(())
        })();
        if self.private_bootstrap {
            self.pending = super::ByteQueue::default();
        }
        self.private_bootstrap = false;
        result
    }
    pub(crate) fn validate_account(&mut self) -> Result<()> {
        let valid = if let Some(codec) = &mut self.claude {
            let valid = codec.check_method(self.expected_method);
            self.account_identity = (self.expected_method == Method::Subscription)
                .then(|| codec.account_identity().map(str::to_owned))
                .flatten();
            valid
        } else {
            let account = self.read_account()?;
            self.account_identity = (self.expected_method == Method::Subscription)
                .then(|| account.account_identity().map(str::to_owned))
                .flatten();
            match self.expected_method {
                Method::Subscription => account.subscription_account(),
                Method::ApiKey => account.api_account(),
            }
        };
        if valid {
            Ok(())
        } else {
            Err(Error::authentication_required(
                "Provider account does not match the selected authentication method.",
            ))
        }
    }
}

fn ephemeral_home(home: &Path) -> Result<()> {
    match std::fs::symlink_metadata(home.join("auth.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::state_io_from(error)),
        Ok(_) => Err(Error::policy_conflict(
            "Codex API login requires an isolated home without persisted credentials; \
                retain the home for inspection.",
        )),
    }
}

use crate::enums::wire_enum;
wire_enum!(CredentialStore { Ephemeral => "ephemeral" });
wire_enum!(ModelProvider { Openai => "openai" });
wire_enum!(LoginMethod { Api => "api" });
wire_enum!(Endpoint { Openai => "https://api.openai.com/v1" });

#[derive(serde::Deserialize)]
struct RequirementsReply {
    #[serde(deserialize_with = "crate::decode::optional_object")]
    requirements: Option<Requirements>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Requirements {
    cli_auth_credentials_store: Option<CredentialStore>,
    model_provider: Option<ModelProvider>,
    allowed_login_methods: Option<Vec<LoginMethod>>,
    #[serde(default, deserialize_with = "crate::decode::optional_object")]
    model_providers: Option<ModelOverrides>,
}
#[derive(serde::Deserialize)]
struct ConfigReply {
    #[serde(deserialize_with = "crate::decode::object")]
    config: Config,
}
#[derive(serde::Deserialize)]
struct Config {
    cli_auth_credentials_store: CredentialStore,
    model_provider: ModelProvider,
    openai_base_url: Option<Endpoint>,
    forced_login_method: Option<LoginMethod>,
    #[serde(default, deserialize_with = "crate::decode::optional_object")]
    model_providers: Option<ModelOverrides>,
}
/// Only presence of an `OpenAI` override matters; its contents are deliberately opaque.
#[derive(serde::Deserialize)]
struct ModelOverrides {
    #[serde(default)]
    openai: crate::runtime::api::Input<serde::de::IgnoredAny>,
}
fn safe_requirements(value: &Value) -> bool {
    crate::decode::object::<_, RequirementsReply>(value).is_ok_and(|reply| {
        reply.requirements.is_none_or(|requirements| {
            requirements
                .cli_auth_credentials_store
                .is_none_or(|v| v == CredentialStore::Ephemeral)
                && requirements
                    .model_provider
                    .is_none_or(|v| v == ModelProvider::Openai)
                && requirements
                    .allowed_login_methods
                    .is_none_or(|methods| methods.contains(&LoginMethod::Api))
                && requirements
                    .model_providers
                    .is_none_or(|providers| providers.openai.is_missing())
        })
    })
}
fn safe_config(value: &Value) -> bool {
    crate::decode::object::<_, ConfigReply>(value).is_ok_and(|reply| {
        let config = reply.config;
        config.cli_auth_credentials_store == CredentialStore::Ephemeral
            && config.model_provider == ModelProvider::Openai
            && config.openai_base_url.is_none_or(|v| v == Endpoint::Openai)
            && config
                .forced_login_method
                .is_none_or(|v| v == LoginMethod::Api)
            && config
                .model_providers
                .is_none_or(|providers| providers.openai.is_missing())
    })
}

#[cfg(test)]
mod original;
#[cfg(test)]
mod tests;
