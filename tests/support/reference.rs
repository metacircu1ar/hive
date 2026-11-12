//! Product-only reference checks shared by Hive and Comb's parser tests.
// Malformed test metadata must fail the test.
#[path = "reference/examples.rs"]
mod examples;
#[path = "reference/labels.rs"]
mod labels;
#[path = "reference/markdown.rs"]
pub mod markdown;
#[path = "reference/scope.rs"]
mod scope;
pub use examples::{argv, examples};
pub use markdown::section;
#[path = "reference/tool_rows.rs"]
pub mod tool_rows;
use clap::Command;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub struct Guide {
    pub name: &'static str,
    pub text: &'static str,
}
pub const GUIDES: &[Guide] = &[
    Guide {
        name: "cli",
        text: include_str!("../../docs/cli-reference.md"),
    },
    Guide {
        name: "setup",
        text: include_str!("../../docs/setup.md"),
    },
    Guide {
        name: "terminal",
        text: include_str!("../../docs/terminal.md"),
    },
    Guide {
        name: "comb",
        text: include_str!("../../comb/README.md"),
    },
    Guide {
        name: "readme",
        text: include_str!("../../README.md"),
    },
    Guide {
        name: "installation",
        text: include_str!("../../docs/platforms-and-installation.md"),
    },
    Guide {
        name: "compatibility",
        text: include_str!("../../docs/compatibility.md"),
    },
    Guide {
        name: "hive-skill",
        text: include_str!("../../skills/hive/SKILL.md"),
    },
    Guide {
        name: "curator-skill",
        text: include_str!("../../skills/comb-curator/SKILL.md"),
    },
];

/// Command paths separated by `|`, guide name, exact Markdown heading.
pub type Anchor = (&'static str, &'static str, &'static str);
/// Hidden command/flag path and its reason. Entries must still be hidden.
pub type Hidden = (&'static str, &'static str);

fn commands(root: &Command) -> BTreeMap<String, Command> {
    fn visit(command: &Command, path: &str, found: &mut BTreeMap<String, Command>) {
        found.insert(path.into(), command.clone());
        for child in command.get_subcommands().filter(|c| c.get_name() != "help") {
            visit(child, format!("{path} {}", child.get_name()).trim(), found);
        }
    }
    let mut found = BTreeMap::new();
    visit(root, "", &mut found);
    found
}

fn flags(command: &Command) -> BTreeMap<String, Vec<String>> {
    command
        .get_arguments()
        .filter(|a| !a.is_hide_set())
        .filter_map(|a| a.get_long().map(|long| (format!("--{long}"), values(a))))
        .collect()
}

fn values(arg: &clap::Arg) -> Vec<String> {
    arg.get_action()
        .takes_values()
        .then(|| arg.get_value_parser().possible_values())
        .flatten()
        .into_iter()
        .flatten()
        .filter(|v| !v.is_hide_set())
        .map(|v| v.get_name().to_owned())
        .collect()
}

fn argument_gaps(command: &Command, text: &str, globals: &BTreeSet<String>) -> Vec<String> {
    let mut errors = Vec::new();
    let mut expected = flags(command);
    expected.retain(|flag, _| !globals.contains(flag));
    for flag in expected.keys() {
        if !mentioned_flags(text).contains(flag) {
            errors.push(format!("missing {flag}"));
        }
    }
    for arg in command
        .get_arguments()
        .filter(|a| !a.is_hide_set() && a.is_positional())
    {
        expected.insert(arg.get_id().to_string(), values(arg));
    }
    for (name, values) in expected {
        for value in values {
            if !mentions(text, &value) {
                errors.push(format!("{name} missing value {value}"));
            }
        }
    }
    errors
}

fn mentioned_flags(text: &str) -> BTreeSet<String> {
    text.split("--")
        .skip(1)
        .filter_map(|tail| {
            let name: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            name.starts_with(|c: char| c.is_ascii_alphabetic())
                .then(|| format!("--{name}"))
        })
        .collect()
}

fn hidden(commands: &BTreeMap<String, Command>, expected: &[Hidden]) {
    let mut actual = BTreeSet::new();
    for (path, command) in commands {
        if command.is_hide_set() {
            actual.insert(path.clone());
            continue;
        }
        for arg in command.get_arguments().filter(|a| a.is_hide_set()) {
            // Inherited globals have one owner, at the root.
            if !path.is_empty() && arg.is_global_set() {
                continue;
            }
            actual.insert(
                format!("{path} --{}", arg.get_long().unwrap())
                    .trim()
                    .to_owned(),
            );
        }
    }
    let expected: BTreeSet<_> = expected
        .iter()
        .map(|(path, reason)| {
            assert!(reason.len() > 30, "hidden plumbing needs a reason: {path}");
            (*path).to_owned()
        })
        .collect();
    assert_eq!(actual, expected, "stale or unclassified hidden plumbing");
}

fn mapped_anchors(
    commands: &BTreeMap<String, Command>,
    anchors: &[Anchor],
) -> BTreeMap<&'static str, (&'static str, &'static str)> {
    let mut mapped = BTreeMap::new();
    for &(paths, guide, heading) in anchors {
        for path in paths.split('|') {
            assert!(
                mapped.insert(path, (guide, heading)).is_none(),
                "duplicate anchor: {path}"
            );
        }
    }
    let actual: BTreeSet<_> = commands
        .iter()
        .filter(|(_, c)| !c.is_hide_set())
        .map(|(p, _)| p.as_str())
        .collect();
    assert_eq!(
        actual,
        mapped.keys().copied().collect(),
        "command anchors are incomplete or stale"
    );
    mapped
}

pub fn check(mut root: Command, anchors: &[Anchor], exclusions: &[Hidden]) -> Vec<String> {
    root.build();
    let commands = commands(&root);
    hidden(&commands, exclusions);
    let mapped = mapped_anchors(&commands, anchors);
    let globals = flags(&root);
    let mut errors = Vec::new();
    for (path, &(guide, heading)) in &mapped {
        let guide = GUIDES.iter().find(|g| g.name == guide).unwrap();
        let text = section(guide.text, heading);
        let name = if path.is_empty() {
            root.get_bin_name().unwrap_or(root.get_name())
        } else {
            path
        };
        if !mentions(text, name) {
            errors.push(format!(
                "{} / {heading}: missing command {name}",
                guide.name
            ));
        }
        let inherited = if path.is_empty() {
            BTreeSet::new()
        } else {
            globals.keys().cloned().collect()
        };
        errors.extend(
            argument_gaps(&commands[*path], text, &inherited)
                .into_iter()
                .map(|gap| format!("{name} / {heading}: {gap}")),
        );
    }

    for &(guide, heading) in mapped.values().collect::<BTreeSet<_>>() {
        let text = section(
            GUIDES.iter().find(|g| g.name == guide).unwrap().text,
            heading,
        );
        let mut allowed: BTreeSet<_> = globals.keys().cloned().collect();
        for (path, anchor) in &mapped {
            // Parent sections include their child sections, so include all commands
            // mapped to a contained section in the same guide.
            if anchor.0 == guide
                && text.contains(section(
                    GUIDES.iter().find(|g| g.name == guide).unwrap().text,
                    anchor.1,
                ))
            {
                allowed.extend(flags(&commands[*path]).into_keys());
            }
        }
        errors.extend(
            scope::stale(&root, text, allowed)
                .into_iter()
                .map(|flag| format!("{guide} / {heading}: unowned or stale {flag}")),
        );
    }
    for guide in GUIDES {
        let bare = match root.get_name() {
            "comb" => guide.name == "comb",
            _ => matches!(guide.name, "cli" | "terminal"),
        };
        errors.extend(
            scope::explicit_flags(&root, guide.text, bare)
                .into_iter()
                .map(|flag| format!("{}: stale explicit {flag}", guide.name)),
        );
    }
    errors
}

fn mentions(text: &str, phrase: &str) -> bool {
    fn words(text: &str) -> Vec<&str> {
        text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .filter(|word| !word.is_empty())
            .collect()
    }
    let phrase = words(phrase);
    words(text).windows(phrase.len()).any(|part| part == phrase)
}

pub fn check_examples(
    executable: &str,
    root: Command,
    absent: &[&str],
    guides: &[Guide],
    parse: impl Fn(&[std::ffi::OsString]) -> Result<(), String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut checked = 0;
    let labels = labels::Labels::new(root, absent);
    for &guide in guides {
        let owner = match guide.name {
            "comb" => "comb",
            "cli" | "terminal" => "hivectl",
            _ => "",
        };
        for snippet in examples(guide).into_iter().filter(|snippet| {
            snippet.text.contains(executable)
                || ((snippet.label || snippet.invocation) && owner == executable)
        }) {
            if snippet.label {
                if let Err(error) = labels.check(executable, owner, &snippet.text) {
                    errors.push(format!("{}: {error}", guide.name));
                }
                continue;
            }
            for words in argv(&snippet.text) {
                let mut words = examples::without_assignments(&words).to_vec();
                let Some(first) = words.first() else {
                    continue;
                };
                // A product name in a prose cell is not an invocation. An
                // explicitly labeled example containing only that name is.
                if words.len() == 1 && snippet.table && !snippet.invocation && !snippet.label {
                    continue;
                }
                if snippet.table
                    && snippet.invocation
                    && owner == executable
                    && !matches!(first.as_str(), "hivectl" | "hive-daemon" | "comb")
                {
                    words.insert(0, executable.to_owned());
                }
                if words.first().is_none_or(|word| word != executable) {
                    continue;
                }
                let args: Vec<_> = words.iter().map(std::ffi::OsString::from).collect();
                checked += 1;
                if let Err(error) = parse(&args) {
                    errors.push(format!("{}: {}: {error}", guide.name, snippet.text));
                }
            }
        }
    }
    assert!(checked > 0, "no examples found for {executable}");
    errors
}

#[test]
fn positional_enum_values_are_required_in_the_command_section() {
    let mut command =
        Command::new("decide").arg(clap::Arg::new("decision").value_parser(["accept", "reject"]));
    command.build();
    assert_eq!(
        argument_gaps(&command, "--help accept", &BTreeSet::new()),
        vec!["decision missing value reject"]
    );
}
