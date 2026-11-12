//! Markdown structure selects declarations; original bytes supply their meaning.
use super::{Diagnostic, EvidenceId, Extraction, Issue, Occurrence, RelationPredicate, Zone};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use std::{collections::BTreeSet, ops::Range};

#[derive(Default)]
struct Item {
    range: Range<usize>,
    paragraphs: usize,
    nested: bool,
    text: Vec<Range<usize>>,
}

struct Section<'a> {
    text: &'a str,
    entry: Option<&'a EvidenceId>,
    verified: bool,
    remaining: usize,
    result: &'a mut Extraction,
}

pub(super) fn append(
    result: &mut Extraction,
    text: &str,
    entry: Option<&EvidenceId>,
    verified: bool,
    remaining: usize,
) {
    Section {
        text,
        entry,
        verified,
        remaining,
        result,
    }
    .parse();
}

impl Section<'_> {
    fn zone(&self) -> Zone {
        if self.entry.is_some() {
            Zone::Timeline
        } else {
            Zone::Compiled
        }
    }

    fn issue(&mut self, range: Range<usize>, issue: Issue) {
        self.result.diagnostic(Diagnostic {
            zone: self.zone(),
            entry: self.entry.cloned(),
            start: range.start,
            end: range.end,
            issue,
        });
    }

    fn parse(&mut self) {
        let first = self.result.occurrences.len();
        let mut stack = Vec::new();
        let mut active = false;
        let mut sections = 0;
        let mut item: Option<Item> = None;
        for (event, range) in Parser::new(self.text).into_offset_iter() {
            match event {
                Event::Start(tag) => {
                    if stack.is_empty() && matches!(tag, Tag::Heading { .. }) {
                        active = matches!(
                            tag,
                            Tag::Heading {
                                level: HeadingLevel::H2,
                                ..
                            }
                        ) && self.text[range.clone()].trim() == "## Relations";
                        if active {
                            sections += 1;
                            if sections > 1 {
                                self.issue(range.clone(), Issue::DuplicateSection);
                            }
                        }
                    }
                    if active && tag == Tag::Item && stack == [TagEnd::List(false)] {
                        item = Some(Item {
                            range: range.clone(),
                            ..Item::default()
                        });
                    } else if let Some(item) = &mut item {
                        item.observe_container(&tag, &stack);
                    }
                    stack.push(tag.to_end());
                }
                Event::End(tag) => {
                    if tag == TagEnd::Item
                        && stack == [TagEnd::List(false), TagEnd::Item]
                        && let Some(item) = item.take()
                    {
                        self.item(item);
                    }
                    stack.pop();
                }
                Event::Text(_) => {
                    // Nested/code blocks already invalidate the item. Never scan
                    // arbitrary container depth for each (possibly lazy) text line.
                    if let Some(item) = &mut item {
                        item.text.push(range);
                    }
                }
                _ => {}
            }
        }
        if sections > 1 {
            // An ambiguous section cannot contribute whichever declarations happened first.
            self.result.occurrences.truncate(first);
        }
    }

    fn item(&mut self, item: Item) {
        if item.nested || item.paragraphs > 1 {
            return;
        }
        let raw = &self.text[item.range.clone()];
        let body = raw
            .trim_start()
            .strip_prefix(['-', '*', '+'])
            .unwrap_or(raw)
            .trim_start();
        if !body.starts_with("[[") {
            return;
        }
        let start = item.range.start + raw.len() - body.len();
        let Some((token, _)) = body[2..].split_once("]]") else {
            self.issue(start..item.range.end, Issue::MalformedToken);
            return;
        };
        let span = start..start + token.len() + 4;
        let Some((predicate, target)) = token.split_once("::") else {
            self.issue(span, Issue::MalformedToken);
            return;
        };
        let Some(predicate) = RelationPredicate::from_token(predicate) else {
            self.issue(span, Issue::UnknownPredicate);
            return;
        };
        let Ok(target) = target.parse() else {
            self.issue(span, Issue::InvalidTarget);
            return;
        };
        if self.remaining == 0 {
            self.issue(span, Issue::OccurrenceLimit);
            return;
        }
        self.remaining -= 1;
        let (references, references_complete) = references(self.text, &item.text);
        self.result.occurrences.push(Occurrence {
            zone: self.zone(),
            entry: self.entry.cloned(),
            start,
            end: span.end,
            predicate,
            target,
            verified: self.verified,
            references,
            references_complete,
        });
    }
}

fn references(text: &str, ranges: &[Range<usize>]) -> (Vec<String>, bool) {
    // The Markdown parser may split adjacent literal text events around brackets.
    // Only coalesce contiguous source ranges, never over code/markup or an escape.
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && last.end == range.start
        {
            last.end = range.end;
        } else {
            merged.push(range.clone());
        }
    }
    let mut refs = BTreeSet::new();
    let mut complete = true;
    for range in merged {
        let raw = &text[range.clone()];
        let mut consumed = 0;
        for (at, _) in raw.match_indices("[evidence:") {
            if at < consumed {
                continue;
            }
            let start = range.start + at;
            let escaped = text.as_bytes()[..start]
                .iter()
                .rev()
                .take_while(|b| **b == b'\\')
                .count()
                % 2
                == 1;
            if escaped {
                continue;
            }
            let Some((id, _)) = raw[at + 10..].split_once(']') else {
                complete = false;
                break;
            };
            // Consume the entire token, including malformed nested openers. No two
            // searches or retained strings cover the same suffix of the annotation.
            consumed = at + 10 + id.len() + 1;
            if id.len() > super::REFERENCE_BYTES || id.contains('[') {
                complete = false;
            } else if !refs.contains(id) {
                if refs.len() == super::REFERENCE_LIMIT {
                    return (refs.into_iter().collect(), false);
                }
                refs.insert(id.to_owned());
            }
        }
    }
    (refs.into_iter().collect(), complete)
}

impl Item {
    fn observe_container(&mut self, tag: &Tag<'_>, stack: &[TagEnd]) {
        if *tag == Tag::Paragraph && stack == [TagEnd::List(false), TagEnd::Item] {
            self.paragraphs += 1;
        } else if matches!(
            tag,
            Tag::List(_)
                | Tag::CodeBlock(_)
                | Tag::BlockQuote(_)
                | Tag::HtmlBlock
                | Tag::Heading { .. }
        ) {
            self.nested = true;
        }
    }
}
