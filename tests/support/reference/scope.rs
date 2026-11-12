//! Attribute option mentions to mapped commands and explicit command references.
use super::examples::{option_tokens, without_assignments};
use super::{flags, mentioned_flags};
use clap::Command;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::collections::BTreeSet;

fn references(text: &str) -> (Vec<(String, bool)>, BTreeSet<String>) {
    let mut references = Vec::new();
    let mut mentions = BTreeSet::new();
    let mut block = None;
    for event in Parser::new_ext(text, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                block = Some(matches!(kind, CodeBlockKind::Fenced(language)
                    if matches!(language.as_ref(), "sh" | "shell" | "bash" | "zsh")));
            }
            Event::End(TagEnd::CodeBlock) => block = None,
            Event::Text(value) if block.is_some() => {
                references.extend(
                    value
                        .replace("\\\n", " ")
                        .lines()
                        .map(|line| (line.to_owned(), block.unwrap())),
                );
            }
            Event::Code(value) => {
                // Unqualified flags belong to the section's commands. Explicit
                // commands (including Git/jq) name their own flag owner instead.
                if value.starts_with("--") {
                    mentions.extend(mentioned_flags(&value));
                }
                references.push((value.into_string(), false));
            }
            _ => {}
        }
    }
    (references, mentions)
}

fn relaxed(command: Command) -> Command {
    let parent = command.get_subcommands().next().is_some();
    command
        .allow_external_subcommands(parent)
        .ignore_errors(true)
        .mut_subcommands(relaxed)
}

fn explicit(root: &Command, text: &str, bare: bool) -> (BTreeSet<String>, BTreeSet<String>) {
    let (references, _) = references(text);
    let executable = root.get_bin_name().unwrap_or(root.get_name());
    let mut stale = BTreeSet::new();
    let mut allowed = BTreeSet::new();
    'references: for (reference, shell_example) in references {
        let Some(words) = shlex::split(&reference) else {
            continue;
        };
        let mut words = without_assignments(&words).to_vec();
        let Some(first) = words.first() else {
            continue;
        };
        if bare && root.find_subcommand(first).is_some() {
            words.insert(0, executable.to_owned());
        } else if first != executable {
            continue;
        }
        let mut selector = relaxed(root.clone());
        let Ok(matches) = selector.try_get_matches_from_mut(words) else {
            continue;
        };
        let mut command = &selector;
        let mut matches = &matches;
        while let Some((name, child)) = matches.subcommand() {
            let Some(selected) = command.find_subcommand(name) else {
                // A shared guide can reference commands absent in this feature
                // build. Their options are not root globals. Complete examples
                // still pass through the full/core real-parser checks.
                continue 'references;
            };
            command = selected;
            matches = child;
        }
        let owned: BTreeSet<_> = flags(command).into_keys().collect();
        // Check every unquoted option, even after an invalid placeholder, enum
        // alternative or bracket. Quoted prompt values cannot become flags.
        // Shell examples are separately parsed through the real CLI, including
        // their pipelines. Do not attribute a following jq option to this command.
        if !shell_example {
            stale.extend(option_tokens(&reference).difference(&owned).cloned());
        }
        allowed.extend(owned);
    }
    (allowed, stale)
}

pub fn explicit_flags(root: &Command, text: &str, bare: bool) -> BTreeSet<String> {
    explicit(root, text, bare).1
}

pub fn stale(root: &Command, text: &str, mut allowed: BTreeSet<String>) -> BTreeSet<String> {
    let (_, mentioned) = references(text);
    let (referenced, mut stale) = explicit(root, text, true);
    allowed.extend(referenced);
    stale.extend(mentioned.difference(&allowed).cloned());
    stale
}

#[test]
fn option_mentions_allow_explicit_cross_references_but_reject_stale_flags() {
    use clap::Arg;
    let mut root = Command::new("demo")
        .subcommand(Command::new("read").arg(Arg::new("after").long("after")))
        .subcommand(Command::new("send").arg(Arg::new("body").long("body")))
        .subcommand(Command::new("get").arg(Arg::new("id").value_parser(clap::value_parser!(u32))));
    root.build();
    assert_eq!(
        stale(
            &root,
            "Use `read` with `--after` or `--gone`.",
            BTreeSet::new()
        ),
        BTreeSet::from(["--gone".into()])
    );
    assert!(
        stale(
            &root,
            "Use `demo send --body message`; `--body` is text. `git diff --ext-d\
        iff` is a Git option.",
            BTreeSet::new()
        )
        .is_empty()
    );
    assert_eq!(
        stale(&root, "Use `demo read --removed`.", BTreeSet::new()),
        BTreeSet::from(["--removed".into()])
    );
    assert_eq!(
        explicit_flags(&root, "`read [--after CURSOR] [--removed]`", true),
        BTreeSet::from(["--removed".into()])
    );
    assert!(explicit_flags(&root, "`send --body '--removed'`", true).is_empty());
    assert_eq!(
        explicit_flags(&root, "`get INVALID_ID [--removed]`", true),
        BTreeSet::from(["--removed".into()])
    );
    assert_eq!(
        explicit_flags(&root, "`LOG=debug demo read --removed`", false),
        BTreeSet::from(["--removed".into()])
    );
}
