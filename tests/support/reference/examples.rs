//! Extract shell examples without executing them or expanding private environment.
use super::Guide;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

pub struct Snippet {
    pub text: String,
    pub table: bool,
    pub label: bool,
    pub invocation: bool,
}

pub fn examples(guide: Guide) -> Vec<Snippet> {
    let mut examples = Vec::new();
    let mut block = None;
    let mut table = Table::default();
    for event in Parser::new_ext(guide.text, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language)))
                if matches!(
                    language.as_ref(),
                    "sh" | "shell" | "bash" | "zsh" | "text" | ""
                ) =>
            {
                block = Some(String::new());
            }
            Event::Text(text) if block.is_some() => block.as_mut().unwrap().push_str(&text),
            Event::End(TagEnd::CodeBlock) if block.is_some() => examples.extend(
                block
                    .take()
                    .unwrap()
                    .replace("\\\n", " ")
                    .lines()
                    .map(|text| Snippet {
                        text: text.into(),
                        table: false,
                        label: false,
                        invocation: true,
                    }),
            ),
            event => {
                if let Some(snippet) = table.observe(event) {
                    examples.push(snippet);
                }
            }
        }
    }
    examples
}

#[derive(Default)]
struct Table {
    active: bool,
    column: usize,
    header: bool,
    headings: Vec<String>,
    cell: String,
}
impl Table {
    fn observe(&mut self, event: Event<'_>) -> Option<Snippet> {
        match event {
            Event::Start(Tag::Table(_)) => {
                self.active = true;
                self.headings.clear();
            }
            Event::End(TagEnd::Table) => self.active = false,
            Event::Start(Tag::TableHead) => {
                self.header = true;
                self.column = 0;
            }
            Event::End(TagEnd::TableHead) => self.header = false,
            Event::Start(Tag::TableRow) => self.column = 0,
            Event::Start(Tag::TableCell) => self.cell.clear(),
            Event::Text(text) if self.active => self.cell.push_str(&text),
            Event::Code(text) if self.active && self.header => self.cell.push_str(&text),
            Event::End(TagEnd::TableCell) => {
                if self.header {
                    self.headings.push(self.cell.clone());
                }
                self.column += 1;
            }
            Event::Code(text) if self.active => {
                let invocation = self.column > 0
                    && self
                        .headings
                        .first()
                        .is_some_and(|s| s.eq_ignore_ascii_case("command"))
                    && (self
                        .headings
                        .get(self.column)
                        .is_some_and(|s| matches!(s.as_str(), "Example" | "Complete example"))
                        || self
                            .cell
                            .trim_end_matches([' ', ':'])
                            .to_ascii_lowercase()
                            .ends_with("example"));
                let snippet = Snippet {
                    text: text.into_string(),
                    table: true,
                    label: self.column == 0
                        && self
                            .headings
                            .first()
                            .is_some_and(|s| s.eq_ignore_ascii_case("command")),
                    invocation,
                };
                self.cell.clear();
                return Some(snippet);
            }
            _ => {}
        }
        None
    }
}

/// Leading shell assignments configure an invocation; never expand their values.
pub fn without_assignments(words: &[String]) -> &[String] {
    let count = words
        .iter()
        .take_while(|word| {
            word.split_once('=').is_some_and(|(name, _)| {
                name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        })
        .count();
    &words[count..]
}

/// All option tokens outside quoted values, including optional synopsis groups.
pub fn option_tokens(text: &str) -> std::collections::BTreeSet<String> {
    let mut unquoted = String::new();
    let mut quote = None;
    let mut escaped = false;
    for c in text.chars() {
        if escaped {
            escaped = false;
            unquoted.push(' ');
        } else if c == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote == Some(c) {
            quote = None;
            unquoted.push(' ');
        } else if quote.is_some() {
            unquoted.push(' ');
        } else if matches!(c, '\'' | '"') {
            quote = Some(c);
            unquoted.push(' ');
        } else {
            unquoted.push(c);
        }
    }
    unquoted
        .split(|c: char| c.is_whitespace() || matches!(c, '[' | ']' | '(' | ')' | '|'))
        .filter_map(|word| word.strip_prefix("--"))
        .map(|word| {
            let name: String = word
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            format!("--{name}")
        })
        .filter(|flag| flag.len() > 2)
        .collect()
}

/// Tokenize shell words, separating pipelines outside quotes. Shell substitutions
/// are opaque example values, never executed. Shell syntax is not a Hive argument.
pub fn argv(line: &str) -> Vec<Vec<String>> {
    let mut shell = Shell::default();
    for character in line.chars() {
        shell.push(character);
    }
    assert!(
        shell.quote.is_none() && shell.depth == 0 && !shell.escape,
        "unfinished shell example: {line}"
    );
    shell.finish();
    shell.commands
}

#[derive(Default)]
struct Shell {
    commands: Vec<Vec<String>>,
    text: String,
    quote: Option<char>,
    escape: bool,
    depth: usize,
    substitution_start: usize,
    comment: bool,
}
impl Shell {
    fn push(&mut self, c: char) {
        if self.comment {
            return;
        }
        if self.escape {
            self.text.push(c);
            self.escape = false;
            return;
        }
        if c == '\\' && self.quote != Some('\'') {
            self.text.push(c);
            self.escape = true;
            return;
        }
        if self.quote == Some(c) {
            self.quote = None;
            self.text.push(c);
            return;
        }
        if self.quote.is_some() {
            self.text.push(c);
            return;
        }
        if matches!(c, '\'' | '"') {
            self.quote = Some(c);
            self.text.push(c);
            return;
        }
        if c == '(' && (self.depth > 0 || self.text.ends_with(['<', '$'])) {
            if self.depth == 0 {
                self.substitution_start = self.text.len() - 1;
            }
            self.depth += 1;
            self.text.push(c);
            return;
        }
        if self.depth > 0 {
            self.text.push(c);
            if c == ')' {
                self.depth -= 1;
                if self.depth == 0 {
                    self.collapse_substitution();
                }
            }
            return;
        }
        match c {
            '#' if self.text.is_empty() || self.text.ends_with(char::is_whitespace) => {
                self.comment = true;
            }
            '|' | ';' | '&' => self.finish(),
            _ => self.text.push(c),
        }
    }
    fn collapse_substitution(&mut self) {
        self.text.truncate(self.substitution_start);
        self.text.push_str("/example/substitution");
    }
    fn finish(&mut self) {
        if self.text.trim().is_empty() {
            return;
        }
        let words = shlex::split(&self.text)
            .unwrap_or_else(|| panic!("invalid shell example: {}", self.text));
        self.commands.push(words);
        self.text.clear();
    }
}

#[test]
fn quoted_arguments_and_process_substitutions_are_not_shell_commands() {
    assert_eq!(
        argv("printf x | hivectl send ID --body 'a | b; c' # note"),
        vec![
            vec!["printf", "x"],
            vec!["hivectl", "send", "ID", "--body", "a | b; c"]
        ]
    );
    assert_eq!(
        argv("hivectl send ID --body-file <(printf 'a ) b')"),
        vec![vec![
            "hivectl",
            "send",
            "ID",
            "--body-file",
            "/example/substitution"
        ]]
    );
    assert_eq!(
        argv("comb search \"$(printf 'two words')\""),
        vec![vec!["comb", "search", "$(printf 'two words')"]]
    );
    assert_eq!(
        argv("comb search $(cat <(printf x))"),
        vec![vec!["comb", "search", "/example/substitution"]]
    );
}

#[test]
fn fenced_blocks_and_command_tables_keep_example_classification() {
    let snippets = examples(Guide {
        name: "fixture",
        text: r"
~~~sh
hivectl agents
~~~

| Command | Complete example |
| --- | --- |
| `send [--help]` | `hivectl send AGENT --body hello` |

~~~ruby
comb search ignored
~~~
",
    });
    let observed: Vec<_> = snippets
        .iter()
        .map(|s| (s.text.as_str(), s.table, s.label, s.invocation))
        .collect();
    assert_eq!(
        observed,
        vec![
            ("hivectl agents", false, false, true),
            ("send [--help]", true, true, false),
            ("hivectl send AGENT --body hello", true, false, true),
        ]
    );
}
