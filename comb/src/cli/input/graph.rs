use super::{Context, Failure, Result};
use comb::{Error, graph::Query, service::Operation};
use comb_protocol::service::input::retrieval::Read;
use hive_input::Code;

pub(in crate::cli) fn selection(
    count: usize,
    after: Option<&str>,
    before: Option<&str>,
) -> Result<()> {
    if !(1..=4).contains(&count) {
        return Err(Context::new("graph-query", "--predicate").invalid(
            Code::OutOfRange,
            "1 to 4 predicates; duplicates are allowed",
        ));
    }
    let parse = |text: Option<&str>, field| {
        text.map(str::parse::<comb::values::Timestamp>)
            .transpose()
            .map_err(|cause| {
                Context::new("graph-query", field)
                    .invalid(Code::InvalidValue, "an RFC3339 timestamp")
                    .caused_by(cause)
            })
    };
    let after = parse(after, "--after")?;
    let before = parse(before, "--before")?;
    comb_protocol::search::date_range(after.as_ref(), before.as_ref()).map_err(|cause| {
        Context::new("graph-query", "--after")
            .invalid(
                Code::ConflictingArguments,
                "a timestamp at or before --before (compared as instants)",
            )
            .caused_by(cause)
    })
}

impl Context<'_> {
    pub(in crate::cli) fn graph(self, text: &str) -> Result<Query> {
        hive_input::validate::byte_count(text.len(), comb::page::PAGE_BYTES)
            .map_err(|rule| self.invalid(rule.code(), rule.expected()))?;
        let value: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text)
            .map_err(|cause| {
                self.invalid(
                    Code::InvalidValue,
                    "a graph-query JSON object with anchor and predicates",
                )
                .caused_by(Error::from(cause))
            })?;
        let operation = Read::GraphQuery
            .tool(&value)
            .map_err(|error| match error.input() {
                Some(input) => self
                    .invalid(input.code(), input.expected())
                    .caused_by(error),
                None => Failure::from(error),
            })?;
        match operation {
            Operation::GraphQuery { request } => Ok(request),
            _ => Err(Error::internal_serialization().into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_json_validates_before_io_and_reports_one_cli_field_without_payloads() {
        let context = Context::new("search", "--graph");
        let query = context
            .graph(r#"{"anchor":"concepts/a","predicates":["depends_on"]}"#)
            .unwrap();
        assert_eq!(query.depth, 2);
        assert!(!query.strict);
        for text in [
            "PRIVATE",
            "null",
            "[]",
            r#"{"anchor":"PRIVATE","predicates":["bogus"]}"#,
        ] {
            let output = context.graph(text).unwrap_err().render();
            assert_eq!(output.exit_code, 2);
            assert!(output.stderr.contains("--graph"));
            assert!(!output.stdout.contains("PRIVATE") && !output.stderr.contains("PRIVATE"));
        }
    }
}
