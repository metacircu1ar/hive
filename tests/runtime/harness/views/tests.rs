mod conversation;
mod joins;
use super::decode;
use hive::config::{Profile, Provider};
use hive::options::{OptionsView, Snapshot};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Named {
    #[serde(default, deserialize_with = "decode::present")]
    name: Option<String>,
}
#[derive(Deserialize)]
struct Nullable {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    thread: Option<String>,
}

#[test]
fn omitted_observations_reject_explicit_null_without_filling_defaults() {
    let missing: Named = decode::object(json!({})).unwrap();
    assert!(missing.name.is_none());
    let named: Named = decode::object(json!({"name":"reviewer"})).unwrap();
    assert_eq!(named.name.as_deref(), Some("reviewer"));
    for malformed in [
        json!({"name":null}),
        json!({"name":false}),
        json!({"name":[]}),
    ] {
        assert!(decode::object::<_, Named>(malformed).is_err());
    }
    let explicit: Nullable = decode::object(json!({"thread":null})).unwrap();
    assert!(explicit.thread.is_none());
    assert!(decode::object::<_, Nullable>(json!({})).is_err());
    assert!(decode::object::<_, Named>(json!([null])).is_err());
}

#[test]
fn public_snapshot_normalization_cannot_hide_wire_changes() {
    let canonical = json!({"values":{}, "sources":{"model":{"source":"role"}}});
    let snapshot: Snapshot = decode::exact_object(canonical.clone()).unwrap();
    assert_eq!(serde_json::to_value(snapshot).unwrap(), canonical);
    let mut explicit_false = canonical;
    explicit_false["sources"]["model"]["native_overlay"] = json!(false);
    assert!(decode::exact_object::<_, Snapshot>(explicit_false).is_err());
    assert!(decode::exact_object::<_, Snapshot>(json!([{}, {}])).is_err());
    let canonical = json!({"options":{}, "option_sources":{}});
    let options: OptionsView = decode::exact_object(canonical.clone()).unwrap();
    assert_eq!(serde_json::to_value(options).unwrap(), canonical);
    let mut explicit_empty = canonical;
    explicit_empty["native_configs"] = json!([]);
    assert!(decode::exact_object::<_, OptionsView>(explicit_empty).is_err());
}

#[test]
fn public_enums_keep_their_literal_string_wire_shape() {
    assert_eq!(
        decode::string_enum::<_, Provider>(json!("codex")).unwrap(),
        Provider::Codex
    );
    for malformed in [json!({"codex":null}), json!(["codex"]), Value::Null] {
        assert!(decode::string_enum::<_, Provider>(malformed).is_err());
    }
    let object_enum = json!({"provider":{"codex":null}, "method":"subscription",
        "api_key_env":null, "api_key_file":null});
    assert!(decode::exact_object::<_, Profile>(object_enum).is_err());
}

#[test]
fn whole_wire_comparisons_preserve_path_spelling_beyond_path_equality() {
    let left = json!({"provider":"codex", "method":"api_key",
        "api_key_env":null, "api_key_file":"/tmp/fixture/file"});
    let right = json!({"provider":"codex", "method":"api_key",
        "api_key_env":null, "api_key_file":"/tmp/fixture//file"});
    let first: Profile = decode::exact_object(left.clone()).unwrap();
    let second: Profile = decode::exact_object(right.clone()).unwrap();
    assert_eq!(first, second);
    assert_eq!(serde_json::to_value(&first).unwrap(), left);
    assert_eq!(serde_json::to_value(&second).unwrap(), right);
    assert_ne!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(second).unwrap()
    );
}

#[test]
fn history_rows_require_objects_and_required_nullable_fields() {
    let wire = json!({"events":[{"seq":1,"agent_id":"a","execution_id":null,
        "timestamp":1,"type":"spawned","data":{},"future":true}],
        "cursor":1,"pruned_through":0,"history_expired":false,"page_extension":7});
    let page: super::EventsView = decode::object(wire.clone()).unwrap();
    assert_eq!(page.events[0].execution_id, None);
    assert_eq!(serde_json::to_value(page).unwrap(), wire);
    let mut missing = wire.clone();
    missing["events"][0]
        .as_object_mut()
        .unwrap()
        .remove("execution_id");
    assert!(decode::object::<_, super::EventsView>(missing).is_err());
    let mut array = wire;
    array["events"][0] = json!([1, "a", null, 1, "spawned", {}]);
    assert!(decode::object::<_, super::EventsView>(array).is_err());
}

#[test]
fn mailbox_rows_preserve_omission_and_nullable_pagination() {
    let wire = json!({"messages":[{"seq":1,"id":"m","sender":"human",
        "recipient":"a","body":"hello","state":"completed","extension":true}],
        "next_before":null,"page_extension":7});
    let page: super::MessagesView = decode::object(wire.clone()).unwrap();
    assert!(page.messages[0].mode.is_none());
    assert_eq!(serde_json::to_value(page).unwrap(), wire);
    let mut missing = wire.clone();
    missing.as_object_mut().unwrap().remove("next_before");
    assert!(decode::object::<_, super::MessagesView>(missing).is_err());
    for key in [
        "mode",
        "kind",
        "delivery_error",
        "not_before",
        "attachments",
        "steering_target",
    ] {
        let mut malformed = wire.clone();
        malformed["messages"][0][key] = Value::Null;
        assert!(decode::object::<_, super::MessagesView>(malformed).is_err());
    }
    let mut array = wire;
    array["messages"][0] = json!([1, "m", "human", "a", "hello", "completed"]);
    assert!(decode::object::<_, super::MessagesView>(array).is_err());
}

#[test]
fn spawn_admission_keeps_its_exact_tag_and_extension_fields() {
    let wire = json!({"status":"accepted","agent_id":"a","message_id":"m",
        "held_for_build":"comb","extension":true});
    let reply: super::SpawnView = decode::object(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    for tag in [json!("running"), json!({"accepted":null}), Value::Null] {
        let mut malformed = wire.clone();
        malformed["status"] = tag;
        assert!(decode::object::<_, super::SpawnView>(malformed).is_err());
    }
    let mut malformed = wire;
    malformed["held_for_build"] = Value::Null;
    assert!(decode::object::<_, super::SpawnView>(malformed).is_err());
}

#[test]
fn redirect_admission_requires_a_nullable_turn_without_defaulting_it() {
    let wire = json!({"message_id":"m","delivery_state":"accepted","recipient":"a",
        "redirect_target":{"execution_id":"e","message_id":"prior","turn_id":null}});
    let reply: super::SendView = decode::object(wire.clone()).unwrap();
    assert!(reply.redirect_target.as_ref().unwrap().turn_id.is_none());
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    let mut missing = wire.clone();
    missing["redirect_target"]
        .as_object_mut()
        .unwrap()
        .remove("turn_id");
    assert!(decode::object::<_, super::SendView>(missing).is_err());
    for target in [Value::Null, json!(["e", "prior", null])] {
        let mut malformed = wire.clone();
        malformed["redirect_target"] = target;
        assert!(decode::object::<_, super::SendView>(malformed).is_err());
    }
}

#[test]
fn list_name_omission_and_tree_name_null_remain_distinct_contracts() {
    let list = json!({"id":"a","state":"stopped","task_outcome":"open",
        "parent_id":null,"role":null,"model":"m","effort":"low","thread_id":null,
        "execution_id":null,"turn_id":null,"control":null,"expired":false,
        "error":null,"lifetime":"independent"});
    let agent: super::listing::ListedAgentView = decode::object(list.clone()).unwrap();
    assert!(agent.name.is_none());
    assert_eq!(serde_json::to_value(agent).unwrap(), list);
    for usage in [Value::Null, json!({"tokens":1})] {
        let mut unexpected = list.clone();
        unexpected["native_usage"] = usage;
        let agent: super::listing::ListedAgentView = decode::object(unexpected.clone()).unwrap();
        assert!(agent.native_usage.is_some());
        assert_eq!(serde_json::to_value(agent).unwrap(), unexpected);
    }
    let mut malformed = list;
    malformed["name"] = Value::Null;
    assert!(decode::object::<_, super::listing::ListedAgentView>(malformed).is_err());
    let tree = json!({"version":{"epoch":1,"revision":2},"truncated":false,"agents":[{
        "id":"a","parent_id":null,"role":null,"name":null,"state":"idle",
        "task_outcome":"open","waiting_reason":null,"auth":"main","auth_group":null,
        "auth_decision":null,"model":"m","effort":"low","sandbox":"workspace-write",
        "one_shot":false,"ended_at":null,"children":0,"queued":0,"mail":0,"approval_id":null}]});
    let page: super::TreeView = decode::object(tree.clone()).unwrap();
    assert!(page.agents[0].name.is_none());
    assert_eq!(serde_json::to_value(page).unwrap(), tree);
    let mut missing = tree;
    missing["agents"][0].as_object_mut().unwrap().remove("name");
    assert!(decode::object::<_, super::TreeView>(missing).is_err());
}
