//! Every public Comb command has a checked section, options and examples.
use super::*;
#[path = "../../../../tests/support/reference.rs"]
mod guide;

const ANCHORS: &[guide::Anchor] = &[
    ("|version", "comb", "Command help and parsing errors"),
    ("setup", "setup", "Comb setup and embedding flags"),
    (
        "service|service install|service start|service stop|service status|service uninstall",
        "setup",
        "Services",
    ),
    (
        "model|model use|model use-http",
        "setup",
        "Comb setup and embedding flags",
    ),
    (
        "serve|mcp|credential|credential create|credential list|credential revoke",
        "comb",
        "Shared serving and MCP",
    ),
    (
        "init|get|put|append|capture|promote|consolidate|decide|rm|list|search|links|backlinks|graph|import|sync|doctor|stats",
        "comb",
        "Core command reference",
    ),
    ("embed", "comb", "Embeddings"),
    ("upgrade", "comb", "Core command reference"),
    (
        "graph-query",
        "comb",
        "Typed graph retrieval in search and reports",
    ),
    ("logs", "comb", "Diagnostic logs"),
];

#[test]
fn cli_reference_covers_commands_flags_values_and_tool_parameters() {
    let mut errors = guide::check(
        schema(),
        ANCHORS,
        &[
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
        ],
    );
    errors.extend(guide::tool_rows::check());
    assert!(errors.is_empty(), "Reference gaps:\n{}", errors.join("\n"));
}

#[test]
fn cli_reference_command_examples_parse() {
    let errors = guide::check_examples("comb", schema(), &[], guide::GUIDES, |args| {
        match parse(args) {
            Ok(_) => Ok(()),
            Err(exit) if exit.exit_code == 0 => Ok(()),
            Err(exit) => Err(exit.stderr),
        }
    });
    assert!(
        errors.is_empty(),
        "Invalid documented examples:\n{}",
        errors.join("\n")
    );
}

#[test]
fn table_examples_and_environment_prefixed_calls_use_the_real_parser() {
    const FIXTURE: &str = "\
| Command | Example |
| --- | --- |
| `list [--limit N]` | `list --limit 0` |
| `list` | `lst --limit 2` |
| `capture TEXT` | `capture` |

| Command | Purpose and example |
| --- | --- |
| `list` | Paginate, for example `lst --limit 2`. |
| `capture TEXT` | Create a note. For example `capture`. |
| `grph SLUG [--depth N] [--gone]` | A stale declaration. |
| `service begin` | A stale nested declaration. |

| Setting | Meaning |
| --- | --- |
| Knowledge | Uses `comb`. |

```sh
COMB_LOG=debug comb list --gone
```
";
    let errors = guide::check_examples(
        "comb",
        schema(),
        &[],
        &[guide::Guide {
            name: "comb",
            text: FIXTURE,
        }],
        |args| parse(args).map(|_| ()).map_err(|exit| exit.stderr),
    );
    assert_eq!(errors.len(), 8, "{errors:?}");
    assert!(errors[0].contains("--limit"));
    assert!(errors[1].contains("lst"));
    assert!(errors[2].contains("capture"));
    assert!(errors[3].contains("lst"));
    assert!(errors[4].contains("capture"));
    assert!(errors[5].contains("grph"));
    assert!(errors[6].contains("service begin"));
    assert!(errors[7].contains("--gone"));
}
