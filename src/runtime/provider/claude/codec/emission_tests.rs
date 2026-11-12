use super::*;
use crate::runtime::provider::claude::tests::Fixture;
use serde_json::json;

#[test]
fn pending_typed_completion_keeps_its_id_and_waits_for_callbacks_and_tools() {
    let mut f = Fixture::new();
    let turn = f.turn();
    f.codec.turn.completion = Some(emitted::TerminalTurn {
        id: turn.clone(),
        status: ProviderTurnStatus::Completed,
    });
    f.codec.turn.model_usage = Some((
        ProviderTurnStatus::Completed,
        crate::runtime::domain::ModelUsage::default(),
    ));
    f.codec
        .callbacks
        .requests
        .insert(50, ("control".into(), json!(7)));
    f.codec.settle();
    assert!(f.codec.pop().is_none());
    assert!(f.codec.turn.completion.is_some());
    f.codec.callbacks.requests.clear();
    f.codec.tools.active.insert("tool".into());
    f.codec.settle();
    assert!(f.codec.pop().is_none());
    f.codec.tools.active.clear();
    f.codec.settle();
    let usage = f.codec.pop().unwrap();
    let frame = crate::runtime::provider::protocol::IncomingFrame::parse(usage);
    assert_eq!(frame.method(), Some(&ServerMethod::ClaudeModelUsage));
    let expected = json!({"method":"turn/completed","params":{
        "threadId":f.codec.config.id,"turn":{"id":turn,"status":"completed"}}});
    assert_eq!(f.codec.pop().unwrap().to_string(), expected.to_string());
    assert!(f.codec.turn.active.is_none());
    assert!(f.codec.turn.completion.is_none());
    assert!(f.codec.pop().is_none());
}
