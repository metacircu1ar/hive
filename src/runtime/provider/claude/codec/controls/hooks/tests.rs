use super::*;
use serde_json::json;

#[test]
fn shell_words_preserve_quotes_newlines_and_substitutions_as_literal_data() {
    for value in [
        "",
        "a'b",
        "line\nnext",
        "é $(exit 99)",
        "'; exit 99; #",
        "a b",
    ] {
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf '%s' {}", shell_word(value))])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, value.as_bytes());
    }
    let command = "printf '%s\\n' 'literal $(exit 99)'\nexit 7";
    let wrapped = supervised_shell("/bin/echo", "token", command);
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &wrapped])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("# {TOKEN_PREFIX}token\n{command}\n")
    );
}

#[test]
fn shell_wrapping_changes_only_the_admitted_command_slot() {
    let mut fixture = crate::runtime::provider::claude::tests::Fixture::new();
    let mut input = json!({"command":"printf hi", "timeout":17, "future":[false, null]});
    fixture
        .codec
        .wrap_shell_command("tool".into(), &mut input, Scope::Parent)
        .unwrap();
    let permit = fixture.codec.tools.shell_permits.get("tool").unwrap();
    assert!(permit.command.starts_with(&format!("# {TOKEN_PREFIX}")));
    assert!(permit.command.contains("\nexec "));
    assert!(permit.command.ends_with("\nprintf hi'"));
    assert_eq!(
        input,
        json!({"command":permit.command, "timeout":17, "future":[false,null]})
    );
    assert!(!permit.permission_used);
    for mut invalid in [json!({}), json!({"command":17}), json!([]), json!(null)] {
        let original = invalid.clone();
        assert!(
            fixture
                .codec
                .wrap_shell_command("missing".into(), &mut invalid, Scope::Parent)
                .is_err()
        );
        assert_eq!(invalid, original);
        assert!(!fixture.codec.tools.shell_permits.contains_key("missing"));
    }
}
