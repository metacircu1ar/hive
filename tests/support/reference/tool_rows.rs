//! Exact tool-name and top-level parameter coverage, independently of CLI flags.
use super::{GUIDES, markdown};
use std::collections::{BTreeMap, BTreeSet};

pub fn check() -> Vec<String> {
    let mut errors = Vec::new();
    for (heading, schemas) in [
        (
            "Hosted agent tools",
            vec![
                include_str!("../../../src/runtime/api/tool_schemas.json"),
                include_str!("../../../src/runtime/api/tool_schemas_core.json"),
            ],
        ),
        (
            "Use from an outside MCP client",
            vec![include_str!("../../../src/external_mcp/tool_schemas.json")],
        ),
    ] {
        let text = markdown::section(GUIDES[0].text, heading);
        let rows = markdown::table_rows(text);
        let mut expected = BTreeMap::<String, BTreeSet<String>>::new();
        for schema in schemas {
            let tools: Vec<serde_json::Value> = serde_json::from_str(schema).unwrap();
            for tool in tools {
                expected
                    .entry(tool["name"].as_str().unwrap().into())
                    .or_default()
                    .extend(
                        tool["inputSchema"]["properties"]
                            .as_object()
                            .unwrap()
                            .keys()
                            .cloned(),
                    );
            }
        }
        let mut actual = BTreeMap::new();
        for row in rows {
            if let Some(name) = row.first().filter(|name| name.starts_with("hive_")) {
                assert!(
                    actual.insert(name.clone(), row[1..].join(" ")).is_none(),
                    "duplicate tool row: {name}"
                );
            }
        }
        for (name, parameters) in &expected {
            let Some(row) = actual.get(name) else {
                errors.push(format!("{heading}: missing {name}"));
                continue;
            };
            let words: BTreeSet<_> = row
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .collect();
            for parameter in parameters {
                if !words.contains(parameter.as_str()) {
                    errors.push(format!("{name}: missing parameter {parameter}"));
                }
            }
        }
        for name in actual.keys().filter(|name| !expected.contains_key(*name)) {
            errors.push(format!("{heading}: stale tool {name}"));
        }
    }
    errors
}
