//! Shared retrieval preparation for socket requests and hosted/stdio tools.
mod fields;
mod metadata;
#[cfg(test)]
mod tests;
use crate::{Result, request::List, search::Search, service::Operation};
use fields::Fields;
pub use metadata::{Read, error};
use serde_json::{Map, Value};

impl Read {
    pub fn tool(self, args: &Map<String, Value>) -> Result<Operation> {
        self.prepare(Fields::new(self, "/arguments", args), false)
    }

    pub fn socket(self, args: &Map<String, Value>) -> Result<Operation> {
        let fields = Fields::new(self, "/operation", args);
        if matches!(self, Self::Search | Self::List | Self::GraphQuery) {
            fields.known(if self == Self::Search {
                &["op", "request", "mode"]
            } else {
                &["op", "request"]
            })?;
            let request = fields.object("request")?;
            let nested = Fields::new(self, "/operation/request", request);
            if self == Self::GraphQuery {
                return Ok(Operation::GraphQuery {
                    request: graph(&nested)?,
                });
            }
            if self == Self::Search {
                let mode = fields.mode()?;
                return Ok(Operation::Search {
                    request: search(&nested)?,
                    mode,
                });
            }
            return Ok(Operation::List {
                request: list(&nested)?,
            });
        }
        self.prepare(fields, true)
    }

    fn prepare(self, fields: Fields<'_>, socket: bool) -> Result<Operation> {
        let mut keys = self.fields().to_vec();
        if socket {
            keys.push("op");
            if self == Self::Links {
                keys.push("incoming");
            }
        }
        fields.known(&keys)?;
        Ok(match self {
            Self::Search => Operation::Search {
                mode: fields.mode()?,
                request: search(&fields)?,
            },
            Self::List => Operation::List {
                request: list(&fields)?,
            },
            Self::Get => Operation::Get {
                slug: fields.slug("slug")?,
            },
            Self::Links | Self::Backlinks => Operation::Links {
                slug: fields.slug("slug")?,
                incoming: if socket {
                    fields.boolean("incoming", false)?
                } else {
                    self == Self::Backlinks
                },
                after: fields.cursor()?,
                limit: fields
                    .number("limit", 1..=crate::limits::LINK_LIMIT_MAX, Some(100), false)?
                    .unwrap_or(100),
            },
            Self::Graph => Operation::Graph {
                slug: fields.slug("slug")?,
                depth: fields
                    .number(
                        "depth",
                        0..=crate::limits::GRAPH_DEPTH_MAX,
                        (!socket).then_some(crate::service::limits::DEFAULT_GRAPH_DEPTH),
                        false,
                    )?
                    .unwrap_or_default(),
            },
            Self::GraphQuery => Operation::GraphQuery {
                request: graph(&fields)?,
            },
        })
    }
}

fn graph(fields: &Fields<'_>) -> Result<crate::graph::Query> {
    fields.known(Read::GraphQuery.fields())?;
    let mut query = crate::graph::Query::new(fields.slug("anchor")?, fields.predicates()?);
    query.to = fields
        .text("to", false)?
        .map(|s| fields.validate("to", s.parse()))
        .transpose()?;
    query.direction = fields.direction()?;
    query.depth = fields
        .number("depth", 0..=4, Some(query.depth), false)?
        .unwrap_or(query.depth);
    query.max_nodes = fields
        .number("max_nodes", 1..=1000, Some(query.max_nodes), false)?
        .unwrap_or(query.max_nodes);
    query.max_edges = fields
        .number("max_edges", 1..=5000, Some(query.max_edges), false)?
        .unwrap_or(query.max_edges);
    query.limit = fields
        .number("limit", 1..=200, Some(query.limit), false)?
        .unwrap_or(query.limit);
    query.max_evidence_bytes = fields
        .number(
            "max_evidence_bytes",
            1..=262_144,
            Some(query.max_evidence_bytes),
            false,
        )?
        .unwrap_or(query.max_evidence_bytes);
    query.deadline_ms = crate::convert::usize_u64(
        fields
            .number("deadline_ms", 1..=500, Some(100), false)?
            .unwrap_or(100),
    );
    query.reviewed_only = fields.boolean("reviewed_only", false)?;
    query.strict = fields.boolean("strict", false)?;
    query.kind = fields.filter("kind")?;
    query.tag = fields.filter("tag")?;
    query.prefix = fields.filter("prefix")?;
    query.after = fields.date("after")?;
    query.before = fields.date("before")?;
    fields.validate(
        "before",
        crate::search::date_range(query.after.as_ref(), query.before.as_ref()),
    )?;
    Ok(query)
}

fn search(fields: &Fields<'_>) -> Result<Search> {
    fields.known(if fields.is_tool() {
        Read::Search.fields()
    } else {
        &[
            "query", "limit", "kind", "tag", "prefix", "after", "before", "graph",
        ]
    })?;
    let query = fields.text("query", true)?.unwrap_or_default();
    fields.validate("query", crate::search::query(query))?;
    let limit = fields
        .number(
            "limit",
            1..=crate::search::limits::SEARCH_LIMIT_MAX,
            Some(crate::search::DEFAULT_SEARCH_LIMIT),
            false,
        )?
        .unwrap_or(crate::search::DEFAULT_SEARCH_LIMIT);
    let after = fields.date("after")?;
    let before = fields.date("before")?;
    fields.validate(
        "before",
        crate::search::date_range(after.as_ref(), before.as_ref()),
    )?;
    Ok(Search {
        query: query.into(),
        limit,
        kind: fields.filter("kind")?,
        tag: fields.filter("tag")?,
        prefix: fields.filter("prefix")?,
        after,
        before,
        graph: fields
            .optional_object("graph")?
            .map(|value| {
                graph(&Fields::new(
                    Read::GraphQuery,
                    if fields.is_tool() {
                        "/arguments/graph"
                    } else {
                        "/operation/request/graph"
                    },
                    value,
                ))
            })
            .transpose()?,
    })
}

fn list(fields: &Fields<'_>) -> Result<List> {
    fields.known(Read::List.fields())?;
    Ok(List {
        kind: fields.filter("kind")?,
        tag: fields.filter("tag")?,
        prefix: fields.filter("prefix")?,
        unverified: fields.boolean("unverified", false)?,
        after: fields.cursor()?,
        limit: fields.number("limit", 1..=crate::limits::LIST_LIMIT_MAX, None, true)?,
    })
}
