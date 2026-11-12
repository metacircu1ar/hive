use super::super::{
    BatchView, SendView,
    joins::{JoinObservation, JoinSnapshot},
};
use super::{Value, decode, json};

fn snapshot() -> Value {
    json!({"join_id":"j","requester":"human","request_id":"r","created_at":1,
        "deadline":null,"state":"completed","members":[{"position":0,"message_id":"m",
        "target":"a","outcome":"completed","output":null,"output_selection":null,
        "output_partial":false,"task_completed":null,"delivered":true,
        "extension":{"retained":true}}],"extension":true})
}

#[test]
fn closed_join_snapshots_preserve_nullable_members_and_complete_replay() {
    let wire = snapshot();
    let reply: JoinSnapshot = decode::object(wire.clone()).unwrap();
    assert!(reply.deadline.is_none());
    assert!(reply.members[0].output.is_none());
    assert!(reply.members[0].output_selection.is_none());
    assert!(reply.members[0].task_completed.is_none());
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    let mut missing = wire.clone();
    missing.as_object_mut().unwrap().remove("deadline");
    assert!(decode::object::<_, JoinSnapshot>(missing).is_err());
    for field in ["output", "output_selection", "task_completed"] {
        let mut missing = wire.clone();
        missing["members"][0].as_object_mut().unwrap().remove(field);
        assert!(decode::object::<_, JoinSnapshot>(missing).is_err());
    }
    let mut malformed = wire;
    malformed["members"][0] = json!([]);
    assert!(decode::object::<_, JoinSnapshot>(malformed).is_err());
}

#[test]
fn join_observations_have_a_required_nullable_snapshot_and_no_fake_closed_fields() {
    let wire = json!({"wait":"timeout","join_id":"j","join":null,"extension":true});
    let reply: JoinObservation = decode::object(wire.clone()).unwrap();
    assert!(reply.join.is_none());
    assert!(reply.error.is_none());
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    assert!(decode::object::<_, JoinSnapshot>(wire.clone()).is_err());
    assert!(decode::object::<_, JoinObservation>(snapshot()).is_err());
    for field in ["wait", "join_id", "join"] {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode::object::<_, JoinObservation>(missing).is_err());
    }
    let mut malformed = wire.clone();
    malformed["error"] = Value::Null;
    assert!(decode::object::<_, JoinObservation>(malformed).is_err());
    let mut observed = wire;
    observed["join"] = snapshot();
    observed["error"] = json!({"code":"timeout","message":"Timed out","extension":7});
    let reply: JoinObservation = decode::object(observed.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), observed);
}

#[test]
fn joined_batch_admission_keeps_member_ids_and_unknown_join_fields() {
    let wire = json!({"results":[{"message_id":"m","recipient":"a",
        "delivery_state":"accepted"}],"join":{"join_id":"j","members":["m"],
        "extension":true}});
    let reply: BatchView<SendView> = decode::object(wire.clone()).unwrap();
    let join = reply.join.as_ref().unwrap();
    assert_eq!(join.join_id, "j");
    assert_eq!(join.members, vec!["m"]);
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    for field in ["join_id", "members"] {
        let mut missing = wire.clone();
        missing["join"].as_object_mut().unwrap().remove(field);
        assert!(decode::object::<_, BatchView<SendView>>(missing).is_err());
    }
}
