//! Keep supported CLI commands, flags, examples and tools covered by product docs.
use super::*;
use crate::reference as guide;

const COMB_COMMANDS: &str = "evidence|collectors|collectors status|collectors discard|collectors release-uncertain|collectors rebind";
const ANCHORS: &[guide::Anchor] = &[
    ("", "cli", "Global options and output"),
    ("config|config check", "cli", "Configuration and capacity"),
    ("setup", "setup", "Hive setup flags"),
    (
        "service|service install|service start|service stop|service status|service uninstall",
        "setup",
        "Services",
    ),
    ("doctor|version", "cli", "Setup, diagnostics and services"),
    (
        "daemon|daemon run|daemon start|daemon restart|daemon stop|daemon status|logs",
        "cli",
        "Daemon",
    ),
    ("state|state status|state upgrade", "cli", "State upgrades"),
    (
        "auth|auth list|auth status|auth login|auth groups|auth usage|models|verify-steering",
        "cli",
        "Authentication and model discovery",
    ),
    (
        "auth recover|auth repair|auth prune|auth generations|auth history-export",
        "cli",
        "Recover, repair, prune, and export",
    ),
    ("spawn", "cli", "Spawn and branch agents"),
    (
        "agents|status|messages|output",
        "cli",
        "Inspect agents, messages, and output",
    ),
    ("read", "cli", "Watch one agent live"),
    ("live", "cli", "Watch live output"),
    ("attach", "terminal", "hivectl attach"),
    ("panes", "terminal", "hivectl panes"),
    ("tui", "terminal", "hivectl tui"),
    ("send", "cli", "Send, steer, redirect, and schedule"),
    ("wait", "cli", "Wait for results and external MCP"),
    ("mcp", "cli", "Use from an outside MCP client"),
    (
        "set-options|set-auth|options|options catalog",
        "cli",
        "Change agent options or login",
    ),
    ("set-name", "cli", "Agent names"),
    (
        "approvals|approvals show|respond",
        "cli",
        "Human questions and approvals",
    ),
    (
        "stop|resume|interrupt|complete|terminate",
        "cli",
        "Stop, resume, interrupt, complete, terminate",
    ),
    ("batch", "cli", "Batch operations"),
    ("purge", "cli", "Delete an ended agent"),
    ("gc", "cli", "Collect old ended agents"),
    ("compact", "cli", "Compact retained history"),
    #[cfg(feature = "comb")]
    (COMB_COMMANDS, "cli", "Shared learning evidence"),
];
const HIDDEN: &[guide::Hidden] = &[
    (
        "--managed-service",
        "Owned service-launch coordination, not a user CLI option.",
    ),
    (
        "setup-maintenance",
        "Installer helper process for owned maintenance; users use setup.",
    ),
    (
        "setup --preserve-stopped-services",
        "Installer coordination preserves explicitly stopped owned services.",
    ),
    (
        "setup --prepare-output",
        "Installer preparation saves complete setup choices before activation.",
    ),
    (
        "setup --prepare-resources",
        "Installer preparation identifies staged and stable setup resources.",
    ),
    (
        "setup --apply-prepared",
        "Installer apply consumes validated setup choices without prompting.",
    ),
    (
        "setup --prepared-sha256",
        "Installer apply binds the exact saved preparation bytes.",
    ),
];

#[test]
fn cli_reference_covers_commands_flags_values_and_tool_parameters() {
    let mut errors = guide::check(schema(false), ANCHORS, HIDDEN);
    errors.extend(guide::tool_rows::check());
    // The globals-only daemon facade uses the already checked root options.
    let mut daemon = schema(true);
    daemon.build();
    let mut root = schema(false);
    root.build();
    for arg in daemon.get_arguments().filter(|a| !a.is_hide_set()) {
        assert!(root.get_arguments().any(|a| a.get_long() == arg.get_long()));
    }
    assert!(guide::section(guide::GUIDES[0].text, "Daemon").contains("hive-daemon"));
    assert!(errors.is_empty(), "Reference gaps:\n{}", errors.join("\n"));
}

#[test]
fn cli_reference_command_examples_parse() {
    let mut errors = Vec::new();
    for (executable, daemon) in [("hivectl", false), ("hive-daemon", true)] {
        let absent: Vec<_> = if !cfg!(feature = "comb") && !daemon {
            COMB_COMMANDS.split('|').collect()
        } else {
            Vec::new()
        };
        errors.extend(guide::check_examples(
            executable,
            schema(daemon),
            &absent,
            guide::GUIDES,
            |args| parse_example(args, daemon),
        ));
    }
    assert!(
        errors.is_empty(),
        "Invalid documented examples:\n{}",
        errors.join("\n")
    );
}

fn parse_example(args: &[OsString], daemon: bool) -> Result<(), String> {
    // The shared guide includes full-build commands. The full suite parses all
    // of them; the core suite additionally proves its absent feature is refused.
    if !cfg!(feature = "comb")
        && !daemon
        && matches!(
            args.get(1).and_then(|arg| arg.to_str()),
            Some("evidence" | "collectors")
        )
    {
        let error = schema(false).try_get_matches_from(args).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        let Err(exit) = parse(args, daemon) else {
            panic!("core accepted a Comb-only command");
        };
        assert_eq!(exit.code, 2);
        return Ok(());
    }
    match parse(args, daemon) {
        Ok(_) => Ok(()),
        Err(exit) if exit.code == 0 => Ok(()),
        Err(exit) => Err(exit.stderr),
    }
}
