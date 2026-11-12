//! Scheduled cli regressions.

#[test]
fn delayed_cli_parses_duration_and_refuses_blocking_send() {
    use clap::Parser;
    let args = [
        "hivectl",
        "send",
        "a",
        "--body-file",
        "message.md",
        "--delay",
        "30m",
    ];
    let parsed = super::super::Cli::try_parse_from(args).unwrap();
    assert!(matches!(
        parsed.command,
        super::super::Command::Send(super::super::send::Send {
            delay: Some(1800),
            ..
        })
    ));
    let mut blocked = args.to_vec();
    blocked.push("--wait");
    assert!(super::super::Cli::try_parse_from(blocked).is_err());
}
