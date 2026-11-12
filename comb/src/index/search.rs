//! Filtered retrieval, ranking and final-hit hydration on the derived index.
use super::limits::IDENTITY_LONG_QUERY_CHARS;
#[cfg(test)]
use crate::Error;
use crate::{
    Result,
    index::Index,
    search::{QueryVector, Search, validate},
    values::{CreateSafety, MatchEvidence, Route, Zone},
};
use std::collections::BTreeMap;

mod bindings;
mod ranking;
mod snippet;
use super::queries::search as sql;
mod vector;
pub(super) use bindings::bind;
use ranking::{Candidate, bonus, order, rrf};
mod graph;
mod hydrate;
mod retrieval;
#[cfg(test)]
pub(crate) use hydrate::HYDRATED;
use snippet::snippet;

#[cfg(test)]
mod filter_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod recency_tests;

fn candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        id: Some(row.get(0)?),
        slug: row.get(1)?,
        zone: if row.get::<_, bool>(2)? {
            Zone::Compiled
        } else {
            Zone::Timeline
        },
        verified: row.get(3)?,
        title_match: row.get(4)?,
        updated: row.get(5)?,
        exact_title: row.get(6)?,
        score: 0.0,
        route: Route::Text,
        evidence: MatchEvidence::KeywordExact,
        safety: CreateSafety::Probable,
    })
}
fn text_candidates(index: &Index, request: &Search) -> Result<Vec<Candidate>> {
    validate(request)?;
    let lower = request.query.to_lowercase();
    let mut pages = BTreeMap::new();
    full_text_candidates(index, request, &lower, &mut pages)?;
    identity_candidates(index, request, &lower, &mut pages)?;
    exact_candidates(index, request, &lower, &mut pages)?;
    let mut values = pages.into_values().collect::<Vec<_>>();
    values.sort_by(order);
    Ok(values)
}
fn full_text_candidates(
    index: &Index,
    request: &Search,
    lower: &str,
    pages: &mut BTreeMap<crate::values::Slug, Candidate>,
) -> Result<()> {
    let query = request
        .query
        .split_whitespace()
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = index.connection.prepare_cached(sql::FTS)?;
    bind(
        &mut statement,
        request.filter(),
        &[(":query", &query), (":lower", &lower)],
    )?;
    let rows = statement.raw_query().mapped(candidate);
    for item in rows {
        let mut hit = item?;
        if pages.contains_key(&hit.slug) {
            continue;
        }
        hit.score = rrf(pages.len());
        pages.insert(hit.slug.clone(), hit);
    }
    Ok(())
}
fn identity_candidates(
    index: &Index,
    request: &Search,
    lower: &str,
    pages: &mut BTreeMap<crate::values::Slug, Candidate>,
) -> Result<()> {
    let identity = if request.query.chars().count() >= IDENTITY_LONG_QUERY_CHARS {
        sql::IDENTITY_LONG
    } else {
        sql::IDENTITY_SHORT
    };
    let identity_query = if request.query.chars().count() >= IDENTITY_LONG_QUERY_CHARS {
        format!("\"{}\"", lower.replace('"', "\"\""))
    } else {
        lower.to_owned()
    };
    let mut statement = index.connection.prepare_cached(identity)?;
    bind(
        &mut statement,
        request.filter(),
        &[(":query", &identity_query), (":lower", &lower)],
    )?;
    for (rank, item) in statement.raw_query().mapped(candidate).enumerate() {
        let mut hit = item?;
        hit.score = rrf(rank);
        hit.evidence = MatchEvidence::IdentitySubstring;
        pages.entry(hit.slug.clone()).or_insert(hit);
    }
    Ok(())
}
fn exact_candidates(
    index: &Index,
    request: &Search,
    lower: &str,
    pages: &mut BTreeMap<crate::values::Slug, Candidate>,
) -> Result<()> {
    let mut statement = index.connection.prepare_cached(sql::EXACT)?;
    bind(
        &mut statement,
        request.filter(),
        &[(":query", &lower), (":lower", &lower)],
    )?;
    for item in statement.raw_query().mapped(candidate) {
        let mut hit = item?;
        hit.score = rrf(0);
        hit.evidence = if hit.exact_title {
            MatchEvidence::ExactTitle
        } else {
            MatchEvidence::Alias
        };
        hit.safety = CreateSafety::Exists;
        pages.insert(hit.slug.clone(), hit);
    }
    Ok(())
}
