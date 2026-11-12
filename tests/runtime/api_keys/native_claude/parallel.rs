//! Borrowed native trace assertions; observations are read by the caller in order.
use super::*;

pub(super) fn tasks(rows: &[Value]) -> &Value {
    let tasks: Vec<_> = rows
        .iter()
        .filter(|row| row["subtype"] == "task_started")
        .collect();
    assert_eq!(tasks.len(), 2, "{rows:?}");
    let mut owners: Vec<_> = tasks
        .iter()
        .map(|task| task["tool_use_id"].as_str().unwrap())
        .collect();
    owners.sort_unstable();
    assert_eq!(owners, ["native-owner-a", "native-owner-b"]);
    let child = tasks
        .iter()
        .find(|task| task["tool_use_id"] == "native-owner-a")
        .unwrap();
    for task in &tasks {
        assert!(task["task_id"].as_str().is_some_and(|id| !id.is_empty()));
        assert_eq!(task["spawn_depth"], 1);
        assert_eq!(task["is_backgrounded"], false);
        assert_eq!(task["task_type"], "local_agent");
        assert!(
            rows.iter()
                .any(|row| row["kind"] == "control" && row["agent_id"] == task["task_id"]),
            "{rows:?}"
        );
    }
    child
}

pub(super) fn dispatch_order(rows: &[Value]) {
    let sdk: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "sdk_dispatch")
        .collect();
    assert_eq!(
        sdk.len(),
        1,
        "native children never dispatched SDK calls: {rows:?}"
    );
    assert_eq!(sdk[0]["tool"], "hive_agents");
    assert_eq!(sdk[0]["empty_arguments"], true);
    let position = |kind| rows.iter().position(|row| row["kind"] == kind).unwrap();
    let overlap = position("overlap_established");
    let denied = position("child_denial_delivered");
    let delivered = position("parent_response_delivered");
    assert!(overlap < denied && denied < delivered);
    let closed: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row["kind"] == "owner_closed")
        .collect();
    assert_eq!(closed.len(), 2);
    assert!(closed.iter().all(|(index, _)| *index > delivered));
}

pub(super) fn permissions(rows: &[Value], child: &Value) {
    assert!(
        rows.iter().any(|row| row["kind"] == "decision"
            && row["tool"] == "mcp__hive__hive_agents"
            && row["tool_id"] == "native-child-sdk"
            && row["agent_id"] == child["task_id"]
            && row["empty_arguments"] == true
            && row["decision"] == "deny"),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row["kind"] == "decision"
            && row["tool"] == "mcp__hive__hive_agents"
            && row["tool_id"] == "native-parent-sdk"
            && row["agent_id"].is_null()
            && row["empty_arguments"] == true
            && row["decision"] == "allow"),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row["kind"] == "decision"
            && row["tool"] == "mcp__docs__lookup"
            && row["agent_id"].is_string()
            && row["decision"] == "allow"),
        "{rows:?}"
    );
    let admitted: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["kind"] == "decision"
                && row["hook"] == "PreToolUse"
                && row["tool"] == "Agent"
                && row["decision"] == "allow"
        })
        .collect();
    assert_eq!(admitted.len(), 2);
    assert!(admitted.iter().all(|row| row["foreground"] == true));
}

pub(super) fn child_requests(rows: &[Value], requests: &[Value], child: &Value) {
    assert_eq!(
        requests
            .iter()
            .filter(|row| row["kind"] == "children_concurrent")
            .count(),
        2
    );
    let first = requests
        .iter()
        .find(|row| row["role"] == "NATIVE_CHILD_A" && row["phase"] == 0)
        .unwrap();
    assert!(
        first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "mcp__docs__lookup"),
        "configured external tool must be available on the child's first request: {first:?}"
    );
    for (tool, tool_id) in [
        ("Agent", "native-child-nested"),
        ("Skill", "native-child-skill"),
    ] {
        if first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == tool)
        {
            assert!(
                rows.iter().any(|row| row["kind"] == "decision"
                    && row["agent_id"] == child["task_id"]
                    && row["tool_id"] == tool_id
                    && row["tool"] == tool
                    && row["decision"] == "deny"),
                "{rows:?}"
            );
        } else {
            assert!(
                requests.iter().any(|row| row["role"] == "NATIVE_CHILD_A"
                    && row["failed_tools"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == tool_id))),
                "native-hidden tool must return a failure: {requests:?}"
            );
        }
    }
    assert!(requests.iter().any(|row| row["role"] == "NATIVE_CHILD_A"));
    assert!(requests.iter().any(|row| row["role"] == "NATIVE_CHILD_B"));
}

pub(super) fn selected_models(requests: &[Value]) {
    let parent_model = &requests
        .iter()
        .find(|row| row["role"] == "NATIVE_ROOT_PARALLEL")
        .unwrap()["model"];
    assert!(
        requests
            .iter()
            .filter(|row| row["kind"] == "request"
                && (row["role"] == "NATIVE_CHILD_A" || row["role"] == "NATIVE_CHILD_B"))
            .all(|row| row["model"].is_string() && &row["model"] != parent_model)
    );
    assert!(
        requests.iter().any(|row| row["external_ok"] == true),
        "{requests:?}"
    );
    assert!(
        requests.iter().any(|row| row["parent_sdk_ok"] == true),
        "{requests:?}"
    );
}
