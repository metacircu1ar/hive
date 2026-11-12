use super::*;
use serde_json::json;

fn reply_parity(id: u64, payload: impl Serialize) {
    let previous = reply(id, api::value(&payload)).to_string();
    let mut frames = Frames::default();
    frames.reply(id, payload);
    assert_eq!(frames.pop_front().unwrap().to_string(), previous);
    assert!(frames.is_empty());
}
fn notice_parity(method: ServerMethod, payload: impl Serialize) {
    let previous = notice(method.clone(), api::value(&payload)).to_string();
    let mut frames = Frames::default();
    frames.notice(method, payload);
    assert_eq!(frames.pop_front().unwrap().to_string(), previous);
}

#[test]
fn translated_frames_keep_exact_bytes_without_the_intermediate_payload_tree() {
    let raw = json!({"z":[null,true,u64::MAX,-0.0],"a":"é\n\"\0"});
    reply_parity(u64::MAX, raw.clone());
    reply_parity(
        7,
        VerifiedBranchTurn {
            turn: Identity { id: "turn" },
            thread: LocatedThread {
                id: "thread",
                path: std::path::Path::new("/é/quoted\"path"),
            },
        },
    );
    notice_parity(
        ServerMethod::ItemStarted,
        ItemParams {
            thread_id: "thread",
            turn_id: "turn",
            item: AssistantItem::DynamicToolCall {
                id: "id".into(),
                tool: "tool".into(),
                arguments: raw,
            },
        },
    );
    notice_parity(
        ServerMethod::ItemCompleted,
        ItemParams {
            thread_id: "thread",
            turn_id: "turn",
            item: AssistantItem::AgentMessage {
                id: None,
                text: "é\n\"\0".repeat(10_000),
                verified_model: true,
            },
        },
    );
    notice_parity(
        ServerMethod::ItemCompleted,
        Summary {
            text: "summary".into(),
            verified_model: false,
            aggregation: "stream",
            native_message_id: None,
        },
    );
    for status in [
        ProviderTurnStatus::Completed,
        ProviderTurnStatus::Interrupted,
        ProviderTurnStatus::Failed,
    ] {
        notice_parity(
            ServerMethod::TurnCompleted,
            ThreadTurn {
                thread_id: "thread",
                turn: TerminalTurn {
                    id: "turn".into(),
                    status,
                },
            },
        );
    }
}

#[test]
fn queue_preserves_fifo_and_the_owned_raw_frame_allocation() {
    let text = "opaque text".repeat(1024);
    let pointer = text.as_ptr();
    let mut frames = Frames::default();
    frames.reply(1, super::super::mcp::Empty {});
    frames.push_back(Value::String(text));
    frames.notice(
        ServerMethod::TurnStarted,
        TurnResult {
            turn: Identity { id: "turn" },
        },
    );
    assert_eq!(
        frames.pop_front().unwrap().to_string(),
        r#"{"id":1,"result":{}}"#
    );
    let Value::String(raw) = frames.pop_front().unwrap() else {
        panic!("raw frame shape")
    };
    assert_eq!(raw.as_ptr(), pointer);
    assert_eq!(
        frames.pop_front().unwrap().to_string(),
        r#"{"method":"turn/started","params":{"turn":{"id":"turn"}}}"#
    );
    assert!(frames.is_empty());
}

#[test]
fn specific_native_frame_encoders_preserve_the_old_full_frame_bytes() {
    let control = super::super::control::OutboundControl {
        request_id: "request",
        request: super::super::control::NativeRequest::initialize(),
    };
    let expected = api::value(&control).to_string();
    assert_eq!(control.into_value().to_string(), expected);
    let user = User {
        uuid: "turn",
        session_id: "session",
        message: UserText {
            content: UserContent::Text("é\n\"\0".into()),
        },
    };
    let expected = api::value(&user).to_string();
    assert_eq!(user.into_value().to_string(), expected);
}

#[test]
fn translated_tool_call_preserves_opaque_arguments_and_request_precision() {
    let data = super::super::control::ToolCall {
        id: u64::MAX,
        method: ServerMethod::ToolCall,
        params: super::super::control::ToolArguments {
            thread_id: "thread",
            turn_id: "turn",
            call_id: "call".into(),
            tool: "hive_read".into(),
            arguments: json!({"text":"é\n\"\0","number":u64::MAX,"raw":[null,7,1.0]}),
        },
    };
    let expected = api::value(&data).to_string();
    let mut frames = Frames::default();
    frames.call(data);
    assert_eq!(frames.pop_front().unwrap().to_string(), expected);
}
