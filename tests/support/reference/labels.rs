//! Command-table labels are declarations, but every command path must resolve.
use super::{commands, examples::option_tokens, flags};
use clap::Command;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub enum LabelError {
    Malformed(String),
    Empty,
    UnexpectedArgument(String),
    UnknownOptions { label: String, options: Vec<String> },
    UnknownCommand(String),
}

impl std::fmt::Display for LabelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(label) => write!(f, "malformed command label: {label}"),
            Self::Empty => f.write_str("empty command label"),
            Self::UnexpectedArgument(label) => {
                write!(f, "unexpected argument in command label: {label}")
            }
            Self::UnknownOptions { label, options } => {
                write!(f, "command label {label}: unknown options {options:?}")
            }
            Self::UnknownCommand(path) => write!(f, "unknown command label: {path}"),
        }
    }
}
impl std::error::Error for LabelError {}

pub struct Labels {
    commands: BTreeMap<String, Command>,
    absent: BTreeSet<String>,
}

#[test]
fn labels_resolve_nested_commands_and_only_exact_feature_absences() {
    let registry = Labels::new(
        Command::new("demo").subcommand(Command::new("daemon").subcommand(Command::new("start"))),
        &["collectors", "collectors status"],
    );
    for valid in ["daemon start", "collectors", "collectors status"] {
        registry.check("demo", "demo", valid).unwrap();
    }
    for invalid in [
        "daemon begin",
        "collectors begin",
        "gone",
        "daemon start [--gone]",
    ] {
        assert!(
            !registry
                .check("demo", "demo", invalid)
                .unwrap_err()
                .to_string()
                .is_empty()
        );
    }
    let facade = Labels::new(Command::new("hive-daemon"), &[]);
    assert!(
        facade
            .check("hive-daemon", "hivectl", "hive-daemon status")
            .unwrap_err()
            .to_string()
            .contains("unexpected argument")
    );
}

impl Labels {
    pub fn new(mut root: Command, absent: &[&str]) -> Self {
        root.build();
        let commands = commands(&root);
        let absent: BTreeSet<String> = absent.iter().map(|path| (*path).to_owned()).collect();
        assert!(
            absent.iter().all(|path| !commands.contains_key(path)),
            "feature-absent command list is stale"
        );
        Self { commands, absent }
    }

    pub fn check(&self, executable: &str, owner: &str, text: &str) -> Result<(), LabelError> {
        let Some(words) = shlex::split(text) else {
            return Err(LabelError::Malformed(text.into()));
        };
        let Some(first) = words.first() else {
            return Err(LabelError::Empty);
        };
        if first.starts_with('/') {
            return Ok(()); // The separate shared slash-registry test owns these.
        }
        let words = if first == executable {
            &words[1..]
        } else if matches!(first.as_str(), "hivectl" | "hive-daemon" | "comb")
            || owner != executable
        {
            return Ok(()); // The other executable's reference check owns it.
        } else {
            &words[..]
        };
        let (path, used) = self.path(words)?;
        let Some(command) = self.commands.get(&path) else {
            // Only exact paths declared absent in this build reach here. The
            // full build checks their arguments; core rejects their invocations.
            assert!(self.absent.contains(&path));
            return Ok(());
        };
        if words
            .get(used)
            .is_some_and(|word| !word.starts_with(['-', '[', '(']))
            && !command.get_arguments().any(clap::Arg::is_positional)
        {
            return Err(LabelError::UnexpectedArgument(text.into()));
        }
        let options: BTreeSet<_> = flags(command).into_keys().collect();
        let unknown: Vec<_> = option_tokens(text).difference(&options).cloned().collect();
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(LabelError::UnknownOptions {
                label: text.into(),
                options: unknown,
            })
        }
    }

    fn path(&self, words: &[String]) -> Result<(String, usize), LabelError> {
        let mut path = String::new();
        let mut used = 0;
        for word in words {
            let prefix = if path.is_empty() {
                String::new()
            } else {
                format!("{path} ")
            };
            let has_children = self.commands.keys().chain(&self.absent).any(|name| {
                name.strip_prefix(&prefix)
                    .is_some_and(|tail| !tail.is_empty() && !tail.contains(' '))
            });
            if !has_children || word.starts_with(['-', '[', '(']) {
                break;
            }
            path = format!("{prefix}{word}");
            if !self.commands.contains_key(&path) && !self.absent.contains(&path) {
                return Err(LabelError::UnknownCommand(path));
            }
            used += 1;
        }
        Ok((path, used))
    }
}
