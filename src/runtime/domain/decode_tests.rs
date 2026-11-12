//! Existing runtime-record shape checks against the shared decoder.
use crate::decode::*;
use crate::runtime::{
    auth_fallback::{Availability, SwitchIntent},
    branching::{AttemptedTurn, FinishedTurn, Intent},
    domain::{OptionsSelection, ReservedTurn},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn strict<T: DeserializeOwned>(good: Value, fields: &[&str]) {
    object::<_, T>(&good).unwrap();
    for key in good.as_object().unwrap().keys() {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(object::<_, T>(&missing).is_err(), "missing {key}");
    }
    let mut unknown = good.clone();
    unknown["unknown"] = json!(true);
    assert!(object::<_, T>(&unknown).is_err());
    let positional = Value::Array(fields.iter().map(|key| good[key].clone()).collect());
    assert!(object::<_, T>(&positional).is_err());
}

#[test]
fn persisted_nested_records_require_current_named_fields() {
    strict::<OptionsSelection>(
        json!({"model":"mock","effort":"low","revision":0,"options":{"values":{},"sources":{}}}),
        &["model", "effort", "revision", "options"],
    );
    strict::<ReservedTurn>(
        json!({"message_id":"m","selection":{"model":"mock","effort":"low","revision":0,"options":{"values":{},"sources":{}}}}),
        &["message_id", "selection"],
    );
    strict::<FinishedTurn>(
        json!({"thread":"t","execution":"e","turn":"u","message":null,"outcome":"completed","at":1}),
        &["thread", "execution", "turn", "message", "outcome", "at"],
    );
    strict::<AttemptedTurn>(
        json!({"message":"m","thread":"t","execution":"e"}),
        &["message", "thread", "execution"],
    );
    strict::<SwitchIntent>(
        json!({"id":uuid::Uuid::nil(),"target":"login","reason":"usage_limit","source_generation":uuid::Uuid::nil(),"source_execution":null,"continuation":null}),
        &[
            "id",
            "target",
            "reason",
            "source_generation",
            "source_execution",
            "continuation",
        ],
    );
    strict::<Intent>(
        json!({"source":"a","source_thread":"t","source_execution":"e","source_turn":"u",
        "phase":"bound","deadline":null,"source_outcome":null,"source_handoff":null,"native_thread":null,"error":null}),
        &[
            "source",
            "source_thread",
            "source_execution",
            "source_turn",
            "phase",
            "deadline",
            "source_outcome",
            "source_handoff",
            "native_thread",
            "error",
        ],
    );
    let availability =
        serde_json::to_value(Availability::available("main", uuid::Uuid::nil())).unwrap();
    strict::<Availability>(
        availability.clone(),
        &[
            "profile",
            "generation",
            "revision",
            "state",
            "until",
            "retry_at",
            "probe_id",
            "probe_agent",
            "probe_message",
            "probe_deadline",
            "probe_error",
            "window",
            "reason",
            "confirmed_at",
            "updated_at",
        ],
    );
    for state in [json!("future"), json!({"available":null})] {
        let mut bad = availability.clone();
        bad["state"] = state;
        assert!(object::<_, Availability>(&bad).is_err());
    }
}

#[test]
fn nested_structs_and_enums_reject_positional_and_object_encodings() {
    assert!(
        object::<_, ReservedTurn>(&json!({"message_id":"m","selection":["mock","low",0]})).is_err()
    );
    for phase in [json!("future"), json!({"bound":null})] {
        let value = json!({"source":"a","source_thread":"t","source_execution":"e","source_turn":"u",
            "phase":phase,"deadline":null,"source_outcome":null,"source_handoff":null,"native_thread":null,"error":null});
        assert!(object::<_, Intent>(&value).is_err());
    }
}
