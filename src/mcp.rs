//! Shared MCP method decision vocabulary.
use crate::enums::wire_enum;

wire_enum!(McpMethod {
    Initialize => "initialize", Initialized => "notifications/initialized", Ping => "ping",
    Cancelled => "notifications/cancelled", ToolsList => "tools/list", ToolsCall => "tools/call",
});
