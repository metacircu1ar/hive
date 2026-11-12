//! Positive native branching uses the exact fixture audit in a libtest child.
use super::{api_fixture::*, *};
#[test]
fn api_branch_and_model_change_keep_native_context_and_selected_key() {
    for provider in [Provider::Codex, Provider::Claude] {
        let (mut f, _) = runtime_fixture(provider);
        f.start();
        let id = f.spawn("reply: inherited fact");
        let parent = startup_finished(&f, &id);
        let branch = f.spawned(json!({"op":"spawn","from_agent":id,"body":"reply: child"}));
        let child = branch.agent_id.as_str();
        let branched = startup_finished(&f, child);
        assert_ne!(branched.thread_id, parent.thread_id);
        assert_eq!(branched.branch_phase.as_deref(), Some("bound"));
        assert_eq!(
            serde_json::to_value(&branched.execution_key).unwrap(),
            serde_json::to_value(&parent.execution_key).unwrap()
        );
        let model = if provider == Provider::Codex {
            "gpt-5.6-sol"
        } else {
            "haiku"
        };
        f.ok(
            json!({"op":"set_options","target":child,"options":{"model":model,"effort":"medium"}}),
        );
        let sent = f.sent(json!({"op":"send","target":child,"body":"reply: changed"}));
        let changed = finished(&f, child, &sent);
        assert_eq!(changed.thread_id, branched.thread_id);
        assert_eq!(changed.model, model);
        assert_eq!(changed.effort, "medium");
        assert_eq!(
            serde_json::to_value(&changed.execution_key).unwrap(),
            serde_json::to_value(&branched.execution_key).unwrap()
        );
        f.shutdown();
        tree_private(&f.root);
    }
}
