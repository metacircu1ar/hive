use super::{Index, Row, Stop, incomplete, sql, stopped};
use crate::{
    Error, Result,
    graph::{Direction, Incomplete, Path, Query, Report},
    search::Filter,
    values::{RelationPredicate, Slug},
};
use rusqlite::params;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
};

pub(super) fn run(
    index: &Index,
    request: &Query,
    scope: Filter<'_>,
    stop: &Stop,
    report: &mut Report,
) -> Result<()> {
    if stopped(report, stop) {
        return Ok(());
    }
    for slug in std::iter::once(&request.anchor).chain(request.to.iter()) {
        if stopped(report, stop) {
            return Ok(());
        }
        if !index.graph_exists(slug)? {
            return Err(Error::not_found());
        }
    }
    let incomplete_sources = index.graph_incomplete_sources(request, scope)?;
    report.coverage.current_incomplete_sources = Some(incomplete_sources);
    if incomplete_sources != 0 {
        incomplete(report, Incomplete::Extraction);
    }
    let Some(anchor) = index.graph_node(request, scope, &request.anchor)? else {
        return Ok(());
    };
    if let Some(to) = &request.to
        && index.graph_node(request, scope, to)?.is_none()
    {
        return Ok(());
    }
    report.nodes.push(anchor);
    report.work.nodes_visited = 1;
    if request.to.as_ref() == Some(&request.anchor) {
        report.connection = Some(Path {
            nodes: vec![request.anchor.clone()],
            edges: Vec::new(),
            unverified: false,
            shortest: report.complete,
        });
        return Ok(());
    }
    let anchor_slug = Rc::new(request.anchor.clone());
    Traversal {
        index,
        request,
        scope,
        stop,
        report,
        visited: BTreeSet::from([Rc::clone(&anchor_slug)]),
        predecessors: BTreeMap::new(),
        queue: VecDeque::from([(anchor_slug, 0)]),
        emitted: BTreeSet::new(),
    }
    .run()
}

struct Traversal<'a> {
    index: &'a Index,
    request: &'a Query,
    scope: Filter<'a>,
    stop: &'a Stop,
    report: &'a mut Report,
    visited: BTreeSet<Rc<Slug>>,
    predecessors: BTreeMap<Rc<Slug>, (Rc<Slug>, Row)>,
    queue: VecDeque<(Rc<Slug>, usize)>,
    emitted: BTreeSet<(Slug, RelationPredicate, Slug)>,
}
impl Traversal<'_> {
    fn run(&mut self) -> Result<()> {
        while let Some((current, depth)) = self.queue.pop_front() {
            if depth == self.request.depth {
                continue;
            }
            if stopped(self.report, self.stop) || !self.expand(&current, depth)? {
                break;
            }
        }
        Ok(())
    }
    fn expand(&mut self, current: &Rc<Slug>, depth: usize) -> Result<bool> {
        let predicates: Vec<_> = self.request.predicates.iter().map(|p| p.as_str()).collect();
        let direction = match self.request.direction {
            Direction::Outgoing => "outgoing",
            Direction::Incoming => "incoming",
            Direction::Both => "both",
        };
        // One lookahead distinguishes an exact bound from omitted work.
        let remaining = crate::convert::usize_u32_capped(
            self.request.max_edges - self.report.work.edges_examined + 1,
        );
        let mut statement = self.index.connection.prepare_cached(sql::ADJACENCY)?;
        let rows = statement.query_map(
            params![
                current.as_ref(),
                predicates.first(),
                predicates.get(1),
                predicates.get(2),
                predicates.get(3),
                self.request.reviewed_only,
                direction,
                remaining
            ],
            Row::decode,
        )?;
        for row in rows {
            if !self.visit(row?, current, depth)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn visit(&mut self, row: Row, current: &Rc<Slug>, depth: usize) -> Result<bool> {
        if stopped(self.report, self.stop) {
            return Ok(false);
        }
        if self.report.work.edges_examined == self.request.max_edges {
            incomplete(self.report, Incomplete::Edges);
            return Ok(false);
        }
        self.report.work.edges_examined += 1;
        if !self.emitted.insert(row.key()) {
            return Ok(true);
        }
        let neighbor = row.neighbor(current.as_ref());
        let Some(node) = self.index.graph_node(self.request, self.scope, neighbor)? else {
            if !self.index.graph_exists(neighbor)? {
                self.report.work.unresolved_targets += 1;
            }
            return Ok(true);
        };
        let unvisited = !self.visited.contains(neighbor);
        if unvisited && self.visited.len() == self.request.max_nodes {
            incomplete(self.report, Incomplete::Nodes);
            return Ok(false);
        }
        if self.request.to.is_none() {
            if self.report.relations.len() == self.request.limit {
                incomplete(self.report, Incomplete::Matches);
                return Ok(false);
            }
            let Some(edge) = self.index.graph_evidence(&row, self.request, self.report)? else {
                return Ok(false);
            };
            self.report.relations.push(edge);
        }
        if unvisited {
            let neighbor = Rc::new(neighbor.clone());
            self.visited.insert(Rc::clone(&neighbor));
            self.report.work.nodes_visited += 1;
            self.report.nodes.push(node);
            self.predecessors
                .insert(Rc::clone(&neighbor), (Rc::clone(current), row));
            if self.request.to.as_ref() == Some(neighbor.as_ref()) {
                self.index.connection_path(
                    neighbor.as_ref(),
                    &self.predecessors,
                    self.request,
                    self.stop,
                    self.report,
                )?;
                return Ok(false);
            }
            self.queue.push_back((neighbor, depth + 1));
        }
        Ok(true)
    }
}
