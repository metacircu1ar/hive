//! Human authority regressions.
use super::*;

#[test]
#[cfg(feature = "comb")]
fn explicit_evidence_replays_and_collector_administration_is_human_only() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    let request =
        json!({"op":"evidence", "kind":"correction", "summary":"Explicitly public correction."});
    let first = call(&mut core, "human", "evidence-1", request.clone()).unwrap();
    assert_eq!(
        first,
        call(&mut core, "human", "evidence-1", request).unwrap()
    );
    assert_eq!(
        call(
            &mut core,
            "human",
            "evidence-1",
            json!({"op":"evidence","kind":"correction","summary":"Different"})
        )
        .unwrap_err()
        .code(),
        "request_conflict"
    );
    for value in [
        json!({"op":"collectors"}),
        json!({"op":"collector_admin","action":"discard","destination":"shared"}),
    ] {
        let request = RequestEnvelope::new(value);
        assert!(
            request
                .request
                .caller_policy(&crate::runtime::hub::Caller::test_agent("agent"))
                .is_err()
        );
    }
}

#[test]
fn live_viewing_is_human_only_and_never_runs_in_the_writer() {
    use crate::runtime::{core::preflight::Facts, hub::Caller};
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    for raw in [
        json!({"op":"live","target":"agent","after":{"epoch":1,"generation":1,"seq":0,"durable":0}}),
        json!({"op":"tree"}),
        json!({"op":"attach","target":"agent"}),
    ] {
        let request = RequestEnvelope::new(raw.clone());
        assert!(
            request.request.caller_policy(&Caller::Human).is_ok(),
            "{raw}"
        );
        for caller in [Caller::test_agent("agent"), Caller::System] {
            assert!(matches!(
                request.request.caller_policy(&caller),
                Err(Error::UnsupportedOperation { .. })
            ));
        }
        // The control server answers these itself; a misrouted one is refused.
        assert!(
            matches!(
                core.dispatch(&Caller::Human, "live", &request, &Facts::default()),
                Err(Error::UnsupportedOperation { .. })
            ),
            "{raw}"
        );
    }
}

#[test]
fn blocking_tools_are_never_advertised_or_accepted_inside_agents() {
    for name in ["hive_call", "hive_wait_message"] {
        assert!(
            !tools()
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name)
        );
        assert!(validate_tool(name, &json!({})).is_err());
    }
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    assert_eq!(
        call(
            &mut core,
            "internal",
            "read",
            json!({"op":"message_result","message_id":"x"})
        )
        .unwrap_err()
        .code(),
        "unsupported_operation"
    );
}

#[test]
fn system_caller_has_only_internal_restoration_permission() {
    use crate::runtime::{core::preflight::Facts, hub::Caller};
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    for raw in [
        json!({"op":"ping"}),
        json!({"op":"shutdown"}),
        json!({"op":"set_auth", "target":"agent", "auth":"main"}),
        json!({"op":"purge", "target":"agent"}),
        json!({"op":"approvals"}),
        json!({"op":"gc", "before":1}),
    ] {
        let request = RequestEnvelope::new(raw);
        assert!(matches!(
            core.dispatch(&Caller::System, "internal", &request, &Facts::default())
                .map(crate::api::value),
            Err(Error::UnsupportedOperation { .. })
        ));
    }
    let restore = Request::LoginRestored(AuthArgs {
        auth: "main".to_owned().into(),
    });
    assert!(restore.caller_policy(&Caller::System).is_ok());
    assert!(restore.caller_policy(&Caller::Human).is_ok());
    assert!(restore.caller_policy(&Caller::test_agent("agent")).is_err());
    assert_eq!(value(Caller::Human), json!("human"));
    assert_eq!(value(Caller::test_agent("agent")), json!("agent"));
}
