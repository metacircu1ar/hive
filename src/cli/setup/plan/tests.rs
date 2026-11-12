use super::*;
use crate::cli::setup::publication::Snapshot;

fn snapshot(text: &str) -> (tempfile::TempDir, Snapshot) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    std::fs::write(&path, text).unwrap();
    let snapshot = Snapshot::inspect(&path).unwrap();
    (temp, snapshot)
}

const CODEX: &str = concat!(
    "auth_profiles:\n  codex-main:\n    provider: codex\n    method: subscription\n",
    "providers:\n  codex:\n    auth: codex-main\n    model: test-model\n    effort: low\n",
    "roles:\n  worker: {}\n",
);

#[test]
fn m5b_provider_choices_preserve_the_question_field_and_conflict_class() {
    let error = parse_provider("SECRET", "--default-provider").unwrap_err();
    assert_eq!(error.code(), "invalid_value");
    assert_eq!(crate::api::value(&error)["field"], "--default-provider");
    assert!(!crate::api::value(error).to_string().contains("SECRET"));
    let (_temp, snapshot) = snapshot(CODEX);
    let options = Values {
        provider: Some(vec![Provider::Codex]),
        default_provider: Some(Provider::Claude),
        ..Values::default()
    };
    let error = Plan::inspect(&snapshot, &options, &mut Terminal::open(true).unwrap())
        .err()
        .unwrap();
    assert_eq!(error.code(), "conflicting_arguments");
    assert_eq!(crate::api::value(error)["field"], "--default-provider");
}

#[test]
fn actual_chosen_default_preserves_implicit_provider_roles_when_adding_a_provider() {
    let (_temp, snapshot) = snapshot(CODEX);
    let options = Values {
        provider: Some(vec![Provider::Codex, Provider::Claude]),
        default_provider: Some(Provider::Codex),
        login: Some(Login::Skip),
        ..Values::default()
    };
    let plan = Plan::inspect(&snapshot, &options, &mut Terminal::open(true).unwrap()).unwrap();
    let resolved = plan.resolved_values(options);
    assert_eq!(resolved.default_provider, Some(Provider::Codex));
    assert_eq!(resolved.codex_profile.as_deref(), Some("codex-main"));
    assert_eq!(resolved.claude_profile.as_deref(), Some("claude-main"));
    assert_eq!(resolved.codex_model.as_deref(), Some("test-model"));
    // Saved real choices are complete even with a disabled terminal.
    let replay = Plan::inspect(&snapshot, &resolved, &mut Terminal::open(true).unwrap()).unwrap();
    assert_eq!(replay.default, Provider::Codex);
    assert_eq!(replay.login, Login::Skip);
    assert_eq!(std::fs::read_to_string(snapshot.target()).unwrap(), CODEX);
}

#[test]
fn completed_choices_do_not_skip_device_or_changed_model_refusals() {
    let both = concat!(
        "auth_profiles:\n  codex-main: {provider: codex, method: subscription}\n",
        "  claude-main: {provider: claude, method: subscription}\n",
    );
    let (_temp, snapshot) = snapshot(both);
    let mut options = Values {
        default_provider: Some(Provider::Codex),
        login: Some(Login::Device),
        ..Values::default()
    };
    let error = Plan::inspect(&snapshot, &options, &mut Terminal::open(true).unwrap())
        .err()
        .unwrap();
    let crate::error::Error::Input { input, .. } = error else {
        panic!("device conflict input")
    };
    assert_eq!(input.field(), "--login");
    options.login = Some(Login::Skip);
    assert!(Plan::inspect(&snapshot, &options, &mut Terminal::open(true).unwrap()).is_ok());
    assert_eq!(std::fs::read_to_string(snapshot.target()).unwrap(), both);
    let (_temp, original) = self::snapshot(CODEX);
    options.provider = Some(vec![Provider::Codex]);
    options.codex_model = Some("different-model".into());
    assert!(Plan::inspect(&original, &options, &mut Terminal::open(true).unwrap()).is_err());
    assert_eq!(std::fs::read_to_string(original.target()).unwrap(), CODEX);
}
