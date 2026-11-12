//! Styled labels and their clickable columns, including a trailing space per label.
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub(super) fn row<T>(
    labels: impl IntoIterator<Item = (T, Span<'static>)>,
) -> (Line<'static>, Vec<(u16, u16, T)>) {
    let (spans, ranges): (Vec<_>, Vec<_>) = labels
        .into_iter()
        .scan(0_u16, |column, (id, label)| {
            let width = u16::try_from(label.content.width()).unwrap_or(u16::MAX);
            let end = column.saturating_add(width);
            let range = (*column, end, id);
            *column = end.saturating_add(1);
            Some(([label, Span::raw(" ")], range))
        })
        .unzip();
    (
        Line::from(spans.into_iter().flatten().collect::<Vec<_>>()),
        ranges,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn columns_keep_unicode_width_gaps_and_saturation() {
        let labels = ["界", "e\u{301}", "", "x"].map(|s| (s, Span::raw(s)));
        let (line, ranges) = row(labels);
        assert_eq!(
            ranges,
            [(0, 2, "界"), (3, 4, "e\u{301}"), (5, 5, ""), (6, 7, "x")]
        );
        assert_eq!(line.to_string(), "界 e\u{301}  x ");
        let (_, ranges) = row([(0, Span::raw("x".repeat(70_000))), (1, Span::raw("x"))]);
        assert_eq!(ranges, [(0, u16::MAX, 0), (u16::MAX, u16::MAX, 1)]);
    }
}
