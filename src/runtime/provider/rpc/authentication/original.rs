//! Frozen S5 authentication predicates.
use super::*;
fn optional_selection(value: Option<&Value>, selected: &str) -> bool {
    value.is_none_or(|value| value.is_null() || value.as_str() == Some(selected))
}
pub(super) fn safe_requirements(value: &Value) -> bool {
    let Some(requirements) = value.get("requirements") else {
        return false;
    };
    requirements.is_null()
        || requirements.is_object()
            && optional_selection(requirements.get("cliAuthCredentialsStore"), "ephemeral")
            && optional_selection(requirements.get("modelProvider"), "openai")
            && requirements.get("allowedLoginMethods").is_none_or(|value| {
                value.is_null()
                    || value.as_array().is_some_and(|methods| {
                        methods.iter().all(Value::is_string)
                            && methods.iter().any(|method| method.as_str() == Some("api"))
                    })
            })
            && requirements.get("modelProviders").is_none_or(|value| {
                value.is_null()
                    || value
                        .as_object()
                        .is_some_and(|map| !map.contains_key("openai"))
            })
}
pub(super) fn safe_config(value: &Value) -> bool {
    let Some(config) = value.get("config").filter(|v| v.is_object()) else {
        return false;
    };
    config
        .get("cli_auth_credentials_store")
        .and_then(Value::as_str)
        == Some("ephemeral")
        && config.get("model_provider").and_then(Value::as_str) == Some("openai")
        && optional_selection(config.get("openai_base_url"), "https://api.openai.com/v1")
        && optional_selection(config.get("forced_login_method"), "api")
        && config.get("model_providers").is_none_or(|value| {
            value.is_null()
                || value
                    .as_object()
                    .is_some_and(|map| !map.contains_key("openai"))
        })
}
