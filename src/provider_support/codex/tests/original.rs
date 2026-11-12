//! Frozen S5 schema decisions, retained as a refactoring oracle.
use super::*;
pub(super) fn inspect(schema: &Value) -> Result<Capabilities> {
    let start = params(schema, "thread/start").ok_or_else(unsupported)?;
    if !object_shape(
        start,
        &[
            "model",
            "cwd",
            "approvalPolicy",
            "sandbox",
            "developerInstructions",
            "config",
            "allowProviderModelFallback",
            "dynamicTools",
        ],
    ) || !field(schema, start, "allowProviderModelFallback", "boolean")
        || !field(schema, start, "dynamicTools", "array")
    {
        return Err(unsupported());
    }
    let tools = resolve(
        schema,
        start
            .pointer("/properties/dynamicTools")
            .ok_or_else(unsupported)?,
    )
    .and_then(|tools| tools.get("items"))
    .and_then(|items| resolve(schema, items))
    .ok_or_else(unsupported)?;
    let tools = function_tool(schema, tools).ok_or_else(unsupported)?;
    if !object_shape(tools, &["type", "name", "description", "inputSchema"])
        || !field(schema, tools, "name", "string")
        || !field(schema, tools, "description", "string")
        || tools
            .pointer("/properties/inputSchema")
            .and_then(|input| resolve(schema, input))
            .is_none_or(|input| !unconstrained_object(input))
    {
        return Err(unsupported());
    }
    for method in [
        "thread/backgroundTerminals/list",
        "thread/backgroundTerminals/clean",
    ] {
        let params = params(schema, method).ok_or_else(unsupported)?;
        if !object_shape(params, &["threadId"]) || !field(schema, params, "threadId", "string") {
            return Err(unsupported());
        }
    }
    let resume = params(schema, "thread/resume").ok_or_else(unsupported)?;
    if !object_shape(
        resume,
        &[
            "model",
            "cwd",
            "approvalPolicy",
            "sandbox",
            "developerInstructions",
            "config",
            "allowProviderModelFallback",
            "threadId",
            "excludeTurns",
        ],
    ) || !field(schema, resume, "threadId", "string")
        || !field(schema, resume, "excludeTurns", "boolean")
    {
        return Err(unsupported());
    }
    Ok(Capabilities {
        branching: supports_fork(schema),
    })
}

pub(super) fn supports_fork(schema: &Value) -> bool {
    params(schema, "thread/fork").is_some_and(|params| {
        object_shape(
            params,
            &[
                "model",
                "cwd",
                "approvalPolicy",
                "sandbox",
                "developerInstructions",
                "config",
                "allowProviderModelFallback",
                "threadId",
                "lastTurnId",
                "excludeTurns",
                "ephemeral",
            ],
        ) && [
            ("threadId", "string"),
            ("lastTurnId", "string"),
            ("excludeTurns", "boolean"),
            ("ephemeral", "boolean"),
        ]
        .iter()
        .all(|(name, kind)| field(schema, params, name, kind))
    })
}

pub(super) fn function_tool<'a>(schema: &'a Value, tools: &'a Value) -> Option<&'a Value> {
    if let Some(variants) = tools.get("oneOf").and_then(Value::as_array) {
        if !tools
            .as_object()?
            .keys()
            .all(|key| matches!(key.as_str(), "oneOf" | "description" | "title" | "default"))
        {
            return None;
        }
        let mut functions = variants
            .iter()
            .filter_map(|variant| resolve(schema, variant))
            .filter(|variant| {
                variant
                    .pointer("/properties/type/enum")
                    .and_then(Value::as_array)
                    .is_some_and(|values| {
                        values.len() == 1 && values[0].as_str() == Some("function")
                    })
            });
        let function = functions.next()?;
        if functions.next().is_some() || !function_discriminator(function) {
            return None;
        }
        Some(function)
    } else {
        // Earlier advertised object schemas may accept the same tagged payload.
        // A contradictory discriminator is never treated as the function shape.
        if !function_discriminator(tools) {
            return None;
        }
        Some(tools)
    }
}

pub(super) fn function_discriminator(tools: &Value) -> bool {
    tools.pointer("/properties/type").is_none_or(|kind| {
        kind.as_object().is_some_and(|object| {
            object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "type" | "enum" | "description" | "title" | "default"
                )
            }) && kind.get("type").and_then(Value::as_str) == Some("string")
                && kind.get("enum").is_none_or(|values| {
                    values.as_array().is_some_and(|values| {
                        values
                            .iter()
                            .any(|value| value.as_str() == Some("function"))
                    })
                })
        })
    })
}

pub(super) fn params<'a>(schema: &'a Value, method: &str) -> Option<&'a Value> {
    let mut matching = schema.get("oneOf")?.as_array()?.iter().filter(|entry| {
        entry
            .pointer("/properties/method/enum")
            .and_then(Value::as_array)
            .is_some_and(|values| values.len() == 1 && values[0].as_str() == Some(method))
    });
    let request = matching.next()?;
    if matching.next().is_some() || !object_shape(request, &["id", "method", "params"]) {
        return None;
    }
    resolve(schema, request.pointer("/properties/params")?)
}
pub(super) fn resolve<'a>(schema: &'a Value, mut node: &'a Value) -> Option<&'a Value> {
    for _ in 0..16 {
        let Some(reference) = node.get("$ref") else {
            return Some(node);
        };
        if !node
            .as_object()?
            .keys()
            .all(|key| matches!(key.as_str(), "$ref" | "description" | "title" | "default"))
        {
            return None;
        }
        // References never cause filesystem or network reads. Only this document's
        // definitions are accepted; missing references and cycles fail closed.
        let reference = reference.as_str()?.strip_prefix("#/definitions/")?;
        if reference.is_empty() || reference.contains(['/', '~']) {
            return None;
        }
        node = schema.get("definitions")?.get(reference)?;
    }
    None
}
// This is a deliberately small schema subset, not a permissive JSON Schema
// interpreter. Unrecognized constraints cannot establish request compatibility.
pub(super) fn object_shape(node: &Value, supplied: &[&str]) -> bool {
    node.as_object().is_some_and(|object| {
        object.keys().all(|key| {
            matches!(
                key.as_str(),
                "type"
                    | "properties"
                    | "required"
                    | "description"
                    | "title"
                    | "default"
                    | "additionalProperties"
            )
        }) && node.get("type").and_then(Value::as_str) == Some("object")
            && node
                .get("additionalProperties")
                .is_none_or(|v| v == &Value::Bool(true))
            && node.get("required").is_none_or(|v| {
                v.as_array().is_some_and(|names| {
                    names
                        .iter()
                        .all(|name| name.as_str().is_some_and(|name| supplied.contains(&name)))
                })
            })
    })
}
pub(super) fn unconstrained_object(node: &Value) -> bool {
    node == &Value::Bool(true)
        || node.as_object().is_some_and(|object| {
            object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "type" | "description" | "title" | "default" | "additionalProperties"
                )
            }) && node
                .get("type")
                .is_none_or(|kind| kind.as_str() == Some("object"))
                && node
                    .get("additionalProperties")
                    .is_none_or(|v| v == &Value::Bool(true))
        })
}
pub(super) fn field(schema: &Value, params: &Value, name: &str, kind: &str) -> bool {
    let Some(node) = params
        .get("properties")
        .and_then(|properties| properties.get(name))
        .and_then(|value| resolve(schema, value))
    else {
        return false;
    };
    let Some(object) = node.as_object() else {
        return false;
    };
    if !object.keys().all(|key| {
        matches!(key.as_str(), "type" | "description" | "title" | "default")
            || (kind == "array" && key == "items")
    }) {
        return false;
    }
    node.get("type").is_some_and(|value| {
        value.as_str() == Some(kind)
            || value
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(kind)))
    })
}
