use super::*;
use serde_json::json;

#[test]
fn authentication_constraints_match_original_presence_and_shape() {
    let values = [
        Value::Null,
        json!(true),
        json!(1),
        json!("wrong"),
        json!("Ephemeral"),
        json!("OPENAI"),
        json!("API"),
        json!("https://api.openai.com/v1/"),
        json!(["unknown", "api"]),
        json!(["api", "unknown"]),
        json!(["unknown"]),
        json!(["api", {}]),
        json!("ephemeral"),
        json!("openai"),
        json!("api"),
        json!("https://api.openai.com/v1"),
        json!([]),
        json!({}),
        json!(["api", null]),
        json!(["api"]),
        json!({"openai":null}),
    ];
    for field in [
        "cliAuthCredentialsStore",
        "modelProvider",
        "allowedLoginMethods",
        "modelProviders",
        "unknown",
    ] {
        for replacement in &values {
            let raw = json!({"requirements":{field:replacement}});
            assert_eq!(
                safe_requirements(&raw),
                original::safe_requirements(&raw),
                "{raw}"
            );
        }
    }
    let good =
        json!({"config":{"cli_auth_credentials_store":"ephemeral","model_provider":"openai"}});
    for field in [
        "cli_auth_credentials_store",
        "model_provider",
        "openai_base_url",
        "forced_login_method",
        "model_providers",
        "unknown",
    ] {
        for replacement in &values {
            let mut raw = good.clone();
            raw["config"][field] = replacement.clone();
            assert_eq!(safe_config(&raw), original::safe_config(&raw), "{raw}");
        }
    }
    for raw in values
        .iter()
        .cloned()
        .chain([json!({}), json!({"requirements":null}), good])
    {
        assert_eq!(
            safe_requirements(&raw),
            original::safe_requirements(&raw),
            "{raw}"
        );
        assert_eq!(safe_config(&raw), original::safe_config(&raw), "{raw}");
        let raw = json!({"requirements":raw,"config":raw});
        assert_eq!(
            safe_requirements(&raw),
            original::safe_requirements(&raw),
            "{raw}"
        );
        assert_eq!(safe_config(&raw), original::safe_config(&raw), "{raw}");
    }
}

#[test]
fn duplicate_config_and_requirement_keys_still_use_the_last_parsed_value() {
    for raw in [
        r#"{"requirements":{"modelProvider":"other","modelProvider":"openai","allowedLoginMethods":["unknown","api"]}}"#,
        r#"{"requirements":{"allowedLoginMethods":["api"],"allowedLoginMethods":["API"]}}"#,
        r#"{"requirements":null,"requirements":{"cliAuthCredentialsStore":[]}}"#,
        r#"{"config":{"cli_auth_credentials_store":"wrong","cli_auth_credentials_store":"ephemeral","model_provider":"openai"}}"#,
        r#"{"config":{"cli_auth_credentials_store":"ephemeral","model_provider":"openai","forced_login_method":"api","forced_login_method":"API"}}"#,
    ] {
        let value: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(safe_config(&value), original::safe_config(&value), "{raw}");
        assert_eq!(
            safe_requirements(&value),
            original::safe_requirements(&value),
            "{raw}"
        );
    }
}
