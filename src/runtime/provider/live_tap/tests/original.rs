//! Frozen S5 live projection, for exact event comparisons.
use super::*;
pub(crate) fn delta_kind(frame: &Value) -> Option<DeltaKind> {
    match frame.get("method")?.as_str()? {
        AGENT_MESSAGE_DELTA => Some(DeltaKind::Shown(Kind::Text)),
        COMMAND_OUTPUT_DELTA => Some(DeltaKind::Shown(Kind::ToolOutput)),
        REASONING_SUMMARY_DELTA => Some(DeltaKind::Shown(Kind::Reasoning)),
        RAW_REASONING_DELTA => Some(DeltaKind::Hidden),
        _ => None,
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A delta from a raw notification; malformed deltas are ignored, never fatal.
pub(crate) fn delta(kind: Kind, frame: &Value) -> Option<Delta> {
    let params = frame.get("params")?;
    Some(Delta {
        thread: string(params, "threadId")?,
        event: Event {
            turn_id: string(params, "turnId"),
            item_id: string(params, "itemId"),
            text: Some(string(params, "delta")?),
            ..Event::new(kind)
        },
    })
}

/// The tool label and start summary for an allowlisted tool item.
fn tool(item: &Value) -> Option<(String, Option<String>)> {
    Some(match item.get("type")?.as_str()? {
        "commandExecution" => (
            "command".into(),
            match item.get("command") {
                Some(Value::String(command)) => Some(command.clone()),
                Some(Value::Array(parts)) => Some(
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            },
        ),
        "fileChange" => (
            "file_change".into(),
            item.get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| change.get("path").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                }),
        ),
        "dynamicToolCall" => (string(item, "tool")?, None),
        "mcpToolCall" => (
            format!("{}/{}", string(item, "server")?, string(item, "tool")?),
            None,
        ),
        _ => return None,
    })
}

/// The live copy of an allowlisted lifecycle frame.
pub(crate) fn event(frame: &IncomingFrame) -> Option<Event> {
    let params = frame.params().raw();
    match frame.method()? {
        method @ (ServerMethod::TurnStarted | ServerMethod::TurnCompleted) => {
            let turn = params.get("turn")?;
            Some(Event {
                turn_id: string(turn, "id"),
                status: string(turn, "status"),
                ..Event::new(if *method == ServerMethod::TurnStarted {
                    Kind::TurnStarted
                } else {
                    Kind::TurnCompleted
                })
            })
        }
        method @ (ServerMethod::ItemStarted | ServerMethod::ItemCompleted) => {
            let item = params.get("item")?;
            let (tool, summary) = tool(item)?;
            let started = *method == ServerMethod::ItemStarted;
            let exit = item
                .get("exitCode")
                .and_then(Value::as_i64)
                .map(|code| format!("exit {code}"));
            Some(Event {
                turn_id: string(params, "turnId"),
                item_id: string(item, "id"),
                tool: Some(tool),
                text: if started { summary } else { exit },
                status: (!started).then(|| string(item, "status")).flatten(),
                ..Event::new(if started {
                    Kind::ToolStarted
                } else {
                    Kind::ToolCompleted
                })
            })
        }
        _ => None,
    }
}
