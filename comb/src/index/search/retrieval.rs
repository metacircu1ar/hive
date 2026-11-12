//! Independent retrieval routes; a graph failure is never a vector fallback.
use super::{Candidate, QueryVector, Search, graph, hydrate::finish, ranking, text_candidates};
use crate::{
    ErrorCode, Result,
    embedding::Coverage,
    graph::Cancellation,
    index::Index,
    search::{Mode, Results, validate},
};

struct Candidates {
    hits: Vec<Candidate>,
    mode: Mode,
    fallback: Option<ErrorCode>,
    coverage: Option<Coverage>,
}
impl Index {
    pub fn search(&self, request: &Search) -> Result<Results> {
        self.retrieve(request, Mode::Text, QueryVector::NotRequested)
    }
    pub fn retrieve(&self, request: &Search, mode: Mode, query: QueryVector) -> Result<Results> {
        self.retrieve_cancellable(request, mode, query, Cancellation::default())
    }
    pub(crate) fn retrieve_cancellable(
        &self,
        request: &Search,
        mode: Mode,
        query: QueryVector,
        cancellation: Cancellation,
    ) -> Result<Results> {
        validate(request)?;
        let mut candidates = self.retrieval_candidates(request, mode, query)?;
        let report = request
            .graph
            .as_ref()
            .map(|graph_request| {
                self.graph_query_in_scope(graph_request, cancellation, Some(request.filter()))
            })
            .transpose()?;
        if let Some(report) = &report {
            // Text-only retrieval retains its old scores unless a new arm is requested.
            if candidates.mode == Mode::Text {
                for (rank, hit) in candidates.hits.iter_mut().enumerate() {
                    hit.score = ranking::rrf(rank);
                }
            }
            candidates.hits = graph::merge(candidates.hits, report);
        }
        Ok(Results {
            results: finish(self, candidates.hits, request)?,
            mode: candidates.mode,
            fallback: candidates.fallback,
            coverage: candidates.coverage,
            graph: report.map(Box::new),
        })
    }

    fn retrieval_candidates(
        &self,
        request: &Search,
        mode: Mode,
        query: QueryVector,
    ) -> Result<Candidates> {
        if mode == Mode::Text {
            return Ok(Candidates {
                hits: text_candidates(self, request)?,
                mode,
                fallback: None,
                coverage: None,
            });
        }
        match query
            .into_result()
            .and_then(|q| self.vector_candidates(request, &q))
        {
            Err(error) if mode == Mode::Hybrid && error.code != ErrorCode::RecoveryRequired => {
                let hits = text_candidates(self, request)?;
                let coverage = Some(self.embedding_coverage(request.filter())?);
                tracing::debug!(name: "comb.search_fallback", code = error.code.as_str(),
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "text_results");
                Ok(Candidates {
                    hits,
                    mode: Mode::Text,
                    fallback: Some(error.code),
                    coverage,
                })
            }
            Err(error) => Err(error),
            Ok((mut vectors, coverage)) => {
                let hits = if mode == Mode::Vector {
                    for (rank, hit) in vectors.iter_mut().enumerate() {
                        hit.score = ranking::rrf(rank);
                    }
                    vectors
                } else {
                    ranking::merge_hybrid(text_candidates(self, request)?, vectors)
                };
                Ok(Candidates {
                    hits,
                    mode,
                    fallback: None,
                    coverage: Some(coverage),
                })
            }
        }
    }
}
