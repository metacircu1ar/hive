//! Human CLI adapter for the same explicit graph request used by tools.
use clap::Args;
use comb_protocol::{
    graph,
    service::Operation,
    values::{RelationPredicate, Slug},
};
use hive_input::clap::parsers;

#[derive(Args)]
pub(super) struct GraphQuery {
    /// Exact source slug. Use --to for a connection, otherwise return neighbors.
    anchor: Slug,
    #[arg(long)]
    to: Option<Slug>,
    /// Repeat for each permitted predicate; no predicates are inferred.
    #[arg(long = "predicate", required = true, value_enum)]
    predicates: Vec<RelationPredicate>,
    #[arg(long, value_enum, default_value = "outgoing")]
    direction: graph::Direction,
    #[arg(long, default_value_t = graph::default_depth(), value_parser = parsers::count(0..=4))]
    depth: usize,
    /// Exclude unreviewed declarations before traversal.
    #[arg(long)]
    reviewed_only: bool,
    /// Refuse incomplete extraction or an exhausted budget instead of returning partial results.
    #[arg(long)]
    strict: bool,
    #[command(flatten)]
    filters: super::args::FilterArgs,
    #[arg(long, value_parser = super::parsing::values::timestamp)]
    after: Option<String>,
    #[arg(long, value_parser = super::parsing::values::timestamp)]
    before: Option<String>,
    #[arg(long, default_value_t = graph::default_nodes(), value_parser = parsers::count(1..=1000))]
    max_nodes: usize,
    #[arg(long, default_value_t = graph::default_edges(), value_parser = parsers::count(1..=5000))]
    max_edges: usize,
    #[arg(long, default_value_t = graph::default_limit(), value_parser = parsers::count(1..=200))]
    limit: usize,
    #[arg(
        long,
        default_value_t = graph::default_evidence(),
        value_parser = parsers::count(1..=262_144),
    )]
    max_evidence_bytes: usize,
    #[arg(long, default_value_t = 100, value_parser = parsers::count(1..=500))]
    deadline_ms: usize,
}
impl GraphQuery {
    pub(super) fn validate_input(&self) -> super::input::Result<()> {
        super::input::graph_selection(
            self.predicates.len(),
            self.after.as_deref(),
            self.before.as_deref(),
        )
    }

    pub(super) fn operation(self) -> comb::Result<Operation> {
        let mut query = graph::Query::new(self.anchor, self.predicates);
        query.to = self.to;
        query.direction = self.direction;
        query.depth = self.depth;
        query.reviewed_only = self.reviewed_only;
        query.strict = self.strict;
        query.kind = self.filters.kind;
        query.tag = self.filters.tag;
        query.prefix = self.filters.prefix;
        query.after = self.after.map(|s| s.parse()).transpose()?;
        query.before = self.before.map(|s| s.parse()).transpose()?;
        query.max_nodes = self.max_nodes;
        query.max_edges = self.max_edges;
        query.limit = self.limit;
        query.max_evidence_bytes = self.max_evidence_bytes;
        query.deadline_ms = comb_protocol::convert::usize_u64(self.deadline_ms);
        query.validate()?;
        Ok(Operation::GraphQuery { request: query })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn cli_graph_query_requires_predicates_and_retains_typed_defaults() {
        assert!(
            crate::cli::args::Cli::try_parse_from(["comb", "graph-query", "concepts/a"]).is_err()
        );
        let cli = crate::cli::args::Cli::try_parse_from([
            "comb",
            "graph-query",
            "concepts/a",
            "--predicate",
            "depends_on",
        ])
        .unwrap();
        let crate::cli::args::RawCommand::GraphQuery(args) = cli.command else {
            panic!("wrong command");
        };
        let Operation::GraphQuery { request } = args.operation().unwrap() else {
            panic!("wrong operation");
        };
        assert_eq!(request.depth, 2);
        assert!(!request.strict && !request.reviewed_only);
        assert_eq!(request.predicates, [RelationPredicate::DependsOn]);
        for option in ["--depth", "--max-nodes", "--max-edges", "--deadline-ms"] {
            assert!(
                crate::cli::args::Cli::try_parse_from([
                    "comb",
                    "graph-query",
                    "concepts/a",
                    "--predicate",
                    "depends_on",
                    option,
                    "100000"
                ])
                .is_err()
            );
        }
    }
}
