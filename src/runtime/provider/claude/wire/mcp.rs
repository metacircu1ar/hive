//! Claude's nested MCP envelopes retain opaque native IDs and tool arguments.
use crate::runtime::api;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct Empty {}
#[derive(Serialize)]
pub(crate) struct Payload<'a> {
    pub mcp_response: Response<'a>,
}
#[derive(Serialize)]
#[serde(tag = "jsonrpc", rename = "2.0")]
pub(crate) struct Response<'a> {
    pub id: Option<&'a Value>,
    #[serde(flatten)]
    pub outcome: Outcome,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Outcome {
    Success { result: ResultBody },
    Failure { error: Failure },
}
#[derive(Serialize)]
pub(crate) struct Failure {
    pub code: i32,
    pub message: &'static str,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum ResultBody {
    Initialize(Initialize),
    Empty(Empty),
    Catalog(Catalog),
    Tool(ToolResult),
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Initialize {
    protocol_version: &'static str,
    capabilities: Capabilities,
    server_info: Server,
}
#[derive(Serialize)]
struct Capabilities {
    tools: Empty,
}
#[derive(Serialize)]
struct Server {
    name: &'static str,
    version: &'static str,
}
#[derive(Serialize)]
pub(crate) struct Catalog {
    pub tools: Vec<Definition>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Definition {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations: Option<Annotations>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Annotations {
    read_only_hint: bool,
    destructive_hint: bool,
    open_world_hint: bool,
}
impl Definition {
    pub(crate) fn native_annotations(&mut self) {
        // This operation reads Hive's local agent listing. The hint permits
        // native parallel scheduling; all ordinary permission gates still run.
        if crate::runtime::api::native_parallel_read(&self.name) {
            self.annotations = Some(Annotations {
                read_only_hint: true,
                destructive_hint: false,
                open_world_hint: false,
            });
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolResult {
    pub content: Vec<Text>,
    pub is_error: bool,
}
#[derive(Serialize)]
#[serde(tag = "type", rename = "text")]
pub(crate) struct Text {
    pub text: String,
}
impl ResultBody {
    pub(crate) fn initialize(protocol_version: &'static str) -> Self {
        Self::Initialize(Initialize {
            protocol_version,
            capabilities: Capabilities { tools: Empty {} },
            server_info: Server {
                name: "hive",
                version: env!("CARGO_PKG_VERSION"),
            },
        })
    }
    pub(crate) const fn empty() -> Self {
        Self::Empty(Empty {})
    }
}
pub(crate) fn reply(control: &str, id: Option<&Value>, outcome: Outcome) -> Value {
    super::control::reply(
        control,
        Payload {
            mcp_response: Response { id, outcome },
        },
    )
}
pub(crate) fn error(control: &str, id: Option<&Value>, code: i32, message: &'static str) -> Value {
    reply(
        control,
        id,
        Outcome::Failure {
            error: Failure { code, message },
        },
    )
}
pub(crate) fn empty() -> Value {
    api::value(Empty {})
}

/// Native Claude currently uses this MCP subset; negotiate its supported date.
pub(crate) fn initialize_protocol(params: &Value) -> Option<&'static str> {
    let requested = params.as_object()?.get("protocolVersion")?.as_str()?;
    hive_input::compatibility::select_mcp(requested, crate::compatibility::CLAUDE_MCP)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copied_registry_hint_keeps_exact_native_annotation_bytes() {
        for name in ["hive_agents", "hive_status", "comb_get", "unknown"] {
            let raw = serde_json::json!({"name":name,"description":"d","inputSchema":{}});
            let mut definition = Definition::deserialize(&raw).unwrap();
            definition.native_annotations();
            let encoded = crate::api::value(&definition);
            if name == "hive_agents" {
                assert_eq!(
                    encoded["annotations"],
                    serde_json::json!({
                        "readOnlyHint":true,"destructiveHint":false,"openWorldHint":false
                    })
                );
                assert_eq!(
                    serde_json::to_string(&definition.annotations).unwrap(),
                    concat!(
                        "{\"readOnlyHint\":true,\"destructiveHint\":false,",
                        "\"openWorldHint\":false}"
                    )
                );
            } else {
                assert_eq!(encoded, raw);
            }
        }
    }
}
