use super::*;
use std::error::Error as _;

fn seeded() -> Report {
    let mut r = report();
    r.checked_at_unix_seconds = 123;
    r.state = Status::RecoveryRequired;
    r.credential_present = Some(false);
    r.observed_method = Some(ObservedMethod::Other);
    r.account_identity = Some("retained".into());
    r.diagnostic = Some(process::malformed());
    r
}

#[test]
fn codex_probe_root_presence_and_required_boolean_keep_distinct_errors_without_mutation() {
    for raw in [
        json!(null),
        json!([]),
        json!(7),
        json!("x"),
        json!({}),
        json!([{"result":{"requiresOpenaiAuth":true}}]),
    ] {
        let mut actual = seeded();
        let error = parse_codex(&raw, &mut actual).unwrap_err();
        assert_eq!(
            error.message(),
            "Account probe failed; provider details were withheld."
        );
        assert!(error.source().is_none());
        assert_eq!(project(&actual), project(seeded()));
    }
    for result in [
        json!(null),
        json!([]),
        json!(7),
        json!({}),
        json!({"requiresOpenaiAuth":null}),
        json!({"requiresOpenaiAuth":0}),
        json!({"requiresOpenaiAuth":"true"}),
        json!([true,{"type":"chatgpt"}]),
    ] {
        let mut actual = seeded();
        let error = parse_codex(&json!({"result":result}), &mut actual).unwrap_err();
        assert_eq!(error.message(), process::malformed().message());
        assert!(error.source().is_none());
        assert_eq!(project(&actual), project(seeded()));
    }
}

#[test]
fn false_auth_requirement_wins_and_only_changes_the_original_report_fields() {
    for account in [
        None,
        Some(json!(null)),
        Some(json!(7)),
        Some(json!([])),
        Some(json!({"type":"unknown"})),
    ] {
        let mut actual = seeded();
        let mut result = json!({"requiresOpenaiAuth":false});
        if let Some(account) = account {
            result["account"] = account;
        }
        parse_codex(&json!({"result":result}), &mut actual).unwrap();
        let mut expected = seeded();
        expected.state = Status::MethodConflict;
        expected.diagnostic = Some(Error::policy_conflict(
            "Provider configuration does not select the required OpenAI authentication.",
        ));
        assert_eq!(project(&actual), project(expected));
    }
}

#[test]
fn account_presence_keeps_credentials_and_other_partial_report_mutations() {
    let mut missing = seeded();
    assert!(parse_codex(&json!({"result":{"requiresOpenaiAuth":true}}), &mut missing).is_err());
    assert_eq!(project(&missing), project(seeded()));
    let mut null = seeded();
    null.credential_present = Some(true);
    parse_codex(
        &json!({"result":{"requiresOpenaiAuth":true,"account":null}}),
        &mut null,
    )
    .unwrap();
    let mut expected = seeded();
    expected.state = Status::Missing;
    expected.credential_present = Some(false);
    assert_eq!(project(null), project(expected));
    for account in [
        json!(7),
        json!([]),
        json!(["chatgpt", "email"]),
        json!({}),
        json!({"type":null}),
        json!({"type":7}),
        json!({"type":"future"}),
    ] {
        let mut actual = seeded();
        let error = parse_codex(
            &json!({"result":{"requiresOpenaiAuth":true,"account":account}}),
            &mut actual,
        )
        .unwrap_err();
        assert_eq!(error.message(), process::malformed().message());
        assert!(error.source().is_none());
        let mut expected = seeded();
        expected.credential_present = Some(true);
        assert_eq!(project(actual), project(expected));
    }
}

#[test]
fn recognized_account_types_and_independent_email_shapes_keep_complete_report_bytes() {
    for kind in ["apiKey", "amazonBedrock"] {
        let mut actual = seeded();
        actual.observed_method = Some(ObservedMethod::Subscription);
        parse_codex(
            &json!({"result":{"requiresOpenaiAuth":true,"account":{"type":kind,"email":7}}}),
            &mut actual,
        )
        .unwrap();
        let mut expected = seeded();
        expected.credential_present = Some(true);
        expected.state = Status::MethodConflict;
        expected.observed_method = Some(ObservedMethod::Other);
        assert_eq!(project(actual), project(expected));
    }
    for email in [
        None,
        Some(json!(null)),
        Some(json!(7)),
        Some(json!([])),
        Some(json!("")),
        Some(json!("é\n\0quoted\"")),
        Some(json!("é".repeat(32_769))),
    ] {
        let identity = email.as_ref().and_then(Value::as_str).map(str::to_owned);
        let mut account = json!({"type":"chatgpt"});
        if let Some(email) = email {
            account["email"] = email;
        }
        let mut actual = seeded();
        parse_codex(
            &json!({"result":{"requiresOpenaiAuth":true,"account":account}}),
            &mut actual,
        )
        .unwrap();
        let mut expected = seeded();
        expected.credential_present = Some(true);
        expected.state = Status::Authenticated;
        expected.observed_method = Some(ObservedMethod::Subscription);
        expected.account_identity = identity;
        assert_eq!(project(actual), project(expected));
    }
}

#[test]
fn account_projection_keeps_duplicate_last_wins_and_key_order() {
    for raw in [
        r#"{"result":{"account":{"email":7,"type":"unknown","type":"chatgpt","email":"last"},"requiresOpenaiAuth":false,"requiresOpenaiAuth":true}}"#,
        r#"{"result":{"requiresOpenaiAuth":true,"account":{"email":"last","type":"chatgpt"}}}"#,
    ] {
        let mut actual = seeded();
        parse_codex(&serde_json::from_str(raw).unwrap(), &mut actual).unwrap();
        let mut expected = seeded();
        expected.credential_present = Some(true);
        expected.state = Status::Authenticated;
        expected.observed_method = Some(ObservedMethod::Subscription);
        expected.account_identity = Some("last".into());
        assert_eq!(project(actual), project(expected));
    }
}
