use crate::Error;
use crate::service::input::samples::{self, ReadExample, RetrievalOperation};
use hive_input::{Code, InputError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Read {
    Search,
    Get,
    List,
    Links,
    Backlinks,
    Graph,
    GraphQuery,
}

impl Read {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "search" => Some(Self::Search),
            "get" => Some(Self::Get),
            "list" => Some(Self::List),
            "links" => Some(Self::Links),
            "backlinks" => Some(Self::Backlinks),
            "graph" => Some(Self::Graph),
            "graph_query" => Some(Self::GraphQuery),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Get => "get",
            Self::List => "list",
            Self::Links => "links",
            Self::Backlinks => "backlinks",
            Self::Graph => "graph",
            Self::GraphQuery => "graph_query",
        }
    }
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Search => &[
                "query", "limit", "kind", "tag", "prefix", "after", "before", "mode", "graph",
            ],
            Self::List => &["kind", "tag", "prefix", "unverified", "after", "limit"],
            Self::Links | Self::Backlinks => &["slug", "after", "limit"],
            Self::Get => &["slug"],
            Self::Graph => &["slug", "depth"],
            Self::GraphQuery => &[
                "anchor",
                "to",
                "predicates",
                "direction",
                "depth",
                "reviewed_only",
                "strict",
                "kind",
                "tag",
                "prefix",
                "after",
                "before",
                "max_nodes",
                "max_edges",
                "limit",
                "max_evidence_bytes",
                "deadline_ms",
            ],
        }
    }
    pub fn sample(self) -> serde_json::Value {
        match self {
            Self::Search => samples::value(ReadExample::Search {
                query: "project architecture",
                mode: "text",
                limit: 10,
            }),
            Self::List => samples::value(ReadExample::List {
                prefix: "concepts/",
                limit: 100,
            }),
            Self::Get => samples::value(ReadExample::Slug {
                slug: "concepts/architecture",
            }),
            Self::Links | Self::Backlinks => samples::value(ReadExample::Links {
                slug: "concepts/architecture",
                limit: 100,
            }),
            Self::Graph => samples::value(ReadExample::Graph {
                slug: "concepts/architecture",
                depth: 2,
            }),
            Self::GraphQuery => samples::value(ReadExample::GraphQuery {
                anchor: "concepts/architecture",
                predicates: ["depends_on"],
                depth: 2,
            }),
        }
    }
    pub fn example(self, tool: bool) -> String {
        let sample = self.sample();
        if tool {
            format!("comb_{} {sample}", self.name())
        } else if matches!(self, Self::Search | Self::List | Self::GraphQuery) {
            let mut sample = sample;
            let mode = sample.as_object_mut().and_then(|map| map.remove("mode"));
            let operation = samples::value(RetrievalOperation {
                op: self.name(),
                request: sample,
                mode,
            });
            format!("operation: {operation}")
        } else {
            let mut sample = sample;
            sample["op"] = self.name().into();
            format!("operation: {sample}")
        }
    }
    pub fn expected(self, field: &str) -> &'static str {
        match field {
            "anchor" | "to" => "an exact canonical page slug in this root",
            "predicates" => {
                "one through four explicit predicates: depends_on, implements, owned_by, related_to"
            }
            "direction" => "outgoing (default), incoming or both; related_to is always symmetric",
            "depth" if self == Self::GraphQuery => "an integer from 0 through 4, default 2",
            "limit" if self == Self::GraphQuery => "an integer from 1 through 200, default 50",
            "max_nodes" => "an integer from 1 through 1000, default 200",
            "max_edges" => "an integer from 1 through 5000, default 1000",
            "max_evidence_bytes" => "an integer from 1 through 262144, default 65536",
            "deadline_ms" => "an integer from 1 through 500, default 100",
            "reviewed_only" | "strict" => "a boolean, default false",
            "query" => "a nonblank query of at most 4096 UTF-8 bytes",
            "graph" => {
                "an optional graph-query object; its filters intersect the search filters at \
                every hop"
            }
            "limit" if self == Self::Search => "an integer from 1 through 100",
            "limit" => "an integer from 1 through 1000",
            "depth" => "an integer from 0 through 8",
            "kind" | "tag" => "a nonempty string of at most 256 UTF-8 bytes",
            "prefix" => "a nonempty literal prefix of at most 240 UTF-8 bytes",
            "after" | "before" if matches!(self, Self::Search | Self::GraphQuery) => {
                "an RFC3339 timestamp, with after no later than before"
            }
            "slug" | "after" => {
                "a page slug: lowercase letters, digits, hyphens or underscores in \
                    slash-separated components; at most 240 bytes total and 100 per component"
            }
            "mode" => "text, vector or hybrid",
            "unverified" | "incoming" => "a boolean",
            "request" => "a retrieval request object",
            _ => "only the documented fields in this object",
        }
    }
}

pub fn error(code: Code, field: &str, expected: &str, example: String) -> Error {
    match InputError::new(
        code,
        "Invalid tool or retrieval input.",
        field,
        expected,
        vec![example],
    ) {
        Ok(input) => input.into(),
        Err(cause) => Error::internal_serialization().caused_by(cause),
    }
}
