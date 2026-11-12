//! Fuse graph rank without claiming lexical identity or semantic similarity.
use super::{Candidate, ranking::rrf};
use crate::{
    graph::Report,
    values::{CreateSafety, MatchEvidence, Route, Zone},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn merge(candidates: Vec<Candidate>, report: &Report) -> Vec<Candidate> {
    let mut pages: BTreeMap<_, _> = candidates
        .into_iter()
        .map(|c| (c.slug.clone(), c))
        .collect();
    // Only cited answers enter search, never nodes visited while seeking a path.
    let cited: BTreeSet<_> = report
        .relations
        .iter()
        .chain(report.connection.iter().flat_map(|p| p.edges.iter()))
        .flat_map(|edge| [&edge.source, &edge.target])
        .collect();
    for (rank, node) in report
        .nodes
        .iter()
        .filter(|n| n.slug != report.request.anchor && cited.contains(&n.slug))
        .enumerate()
    {
        let contribution = rrf(rank);
        if let Some(prior) = pages.get_mut(&node.slug) {
            prior.score += contribution;
            prior.route = match prior.route {
                Route::Text => Route::TextGraph,
                Route::Vector => Route::VectorGraph,
                Route::TextVector => Route::TextVectorGraph,
                other => other,
            };
        } else {
            pages.insert(
                node.slug.clone(),
                Candidate {
                    id: None,
                    slug: node.slug.clone(),
                    score: contribution,
                    zone: Zone::Compiled,
                    verified: !node.page_unverified,
                    title_match: false,
                    exact_title: false,
                    updated: None,
                    route: Route::Graph,
                    evidence: MatchEvidence::Relation,
                    safety: CreateSafety::Unknown,
                },
            );
        }
    }
    pages.into_values().collect()
}
