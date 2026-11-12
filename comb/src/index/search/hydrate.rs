//! Hydrate only final ranked pages, after route fusion and existing bonuses.
use super::{Candidate, Search, bonus, order, snippet, sql};
use crate::{
    Result,
    index::{
        Index,
        limits::{HEADING_CHARS, SNIPPET_CHARS},
    },
    search::Hit,
    values::Prefix,
};

#[cfg(test)]
thread_local! { pub(crate) static HYDRATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub(super) fn finish(
    index: &Index,
    mut candidates: Vec<Candidate>,
    request: &Search,
) -> Result<Vec<Hit>> {
    let now = chrono::Utc::now().timestamp();
    for hit in &mut candidates {
        let half_life = (hit.verified && request.after.is_none() && request.before.is_none())
            .then(|| Prefix::half_life_days(&hit.slug));
        bonus(hit, half_life, now);
    }
    candidates.sort_by(order);
    candidates.truncate(request.limit);
    let mut statement = index.connection.prepare_cached(sql::HYDRATE)?;
    let mut graph_statement = candidates
        .iter()
        .any(|c| c.id.is_none())
        .then(|| index.connection.prepare_cached(sql::HYDRATE_GRAPH))
        .transpose()?;
    candidates
        .into_iter()
        .map(|c| {
            #[cfg(test)]
            HYDRATED.set(HYDRATED.get() + 1);
            let decode = |r: &rusqlite::Row<'_>| {
                Ok(Hit {
                    slug: c.slug.clone(),
                    title: r.get(0)?,
                    kind: r.get(1)?,
                    snippet: r.get(2)?,
                    heading: r.get(3)?,
                    zone: r.get(4)?,
                    unverified: !r.get::<_, bool>(5)?,
                    page_unverified: !r.get::<_, bool>(6)?,
                    score: c.score,
                    matched_by: c.route,
                    evidence: c.evidence,
                    create_safety: c.safety,
                    highlights: Vec::new(),
                })
            };
            let hit = match c.id {
                Some(id) => statement.query_row([id], decode)?,
                None => graph_statement
                    .as_mut()
                    .ok_or_else(crate::Error::graph_index_invalid)?
                    .query_row(
                        rusqlite::params![&c.slug, crate::convert::usize_u32_capped(SNIPPET_CHARS)],
                        decode,
                    )?,
            };
            Ok(search_preview(hit, &request.query, c.id.is_some()))
        })
        .collect()
}

fn search_preview(mut hit: Hit, query: &str, lexical: bool) -> Hit {
    if lexical {
        let snippet = snippet(&hit.snippet, query);
        hit.snippet = snippet.text;
        hit.highlights = snippet.highlights;
    }
    // Graph-only pages have no lexical match to locate. SQL supplies a
    // bounded Unicode preview; never scan a whole page after graph's budget.
    // The full path remains in get; bound search presentation even for a
    // pathological megabyte-long Markdown heading.
    if hit.heading.chars().count() > HEADING_CHARS {
        hit.heading = hit
            .heading
            .chars()
            .take(HEADING_CHARS)
            .chain(std::iter::once('…'))
            .collect();
    }
    hit
}
