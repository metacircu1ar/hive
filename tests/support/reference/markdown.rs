//! Heading boundaries and table cells, parsed as Markdown rather than raw lines.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub fn section<'a>(text: &'a str, wanted: &str) -> &'a str {
    let mut headings = Vec::new();
    let mut current = None;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((level, range.start, String::new()));
            }
            Event::Text(value) | Event::Code(value) => {
                if let Some((_, _, title)) = &mut current {
                    title.push_str(&value);
                }
            }
            Event::End(TagEnd::Heading(_)) => headings.push(current.take().unwrap()),
            _ => {}
        }
    }
    let selected: Vec<_> = headings
        .iter()
        .enumerate()
        .filter(|(_, (_, _, title))| title == wanted)
        .collect();
    assert_eq!(selected.len(), 1, "missing or ambiguous heading: {wanted}");
    let (index, (level, start, _)) = selected[0];
    let end = headings[index + 1..]
        .iter()
        .find(|(next, _, _)| next <= level)
        .map_or(text.len(), |(_, offset, _)| *offset);
    &text[*start..end]
}

pub fn table_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = None;
    let mut cell = None;
    for event in Parser::new_ext(text, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::TableRow) => row = Some(Vec::new()),
            Event::Start(Tag::TableCell) => cell = Some(String::new()),
            Event::Text(value) | Event::Code(value) if cell.is_some() => {
                cell.as_mut().unwrap().push_str(&value);
            }
            Event::SoftBreak | Event::HardBreak if cell.is_some() => {
                cell.as_mut().unwrap().push(' ');
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(row) = &mut row {
                    row.push(cell.take().unwrap());
                } else {
                    cell = None;
                }
            }
            Event::End(TagEnd::TableRow) => rows.push(row.take().unwrap()),
            _ => {}
        }
    }
    rows
}
