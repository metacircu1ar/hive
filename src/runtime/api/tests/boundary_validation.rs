//! Boundary validation regressions.
use super::*;

#[test]
fn boundary_inputs_preserve_json_type_leniency_and_byte_limits() {
    for raw in [
        Value::Null,
        json!(false),
        json!(true),
        json!(-1),
        json!(1.0),
        json!("1"),
        json!([]),
        json!({}),
    ] {
        assert_eq!(
            Input::<u64>::from_value(raw.clone()).copied(),
            raw.as_u64(),
            "{raw}"
        );
        assert_eq!(
            Input::<bool>::from_value(raw.clone()).copied() == Some(true),
            raw == true,
            "{raw}"
        );
        assert_eq!(
            Input::<String>::from_value(raw.clone()).as_str(),
            raw.as_str(),
            "{raw}"
        );
    }
    for number in [0, 1, i64::MAX as u64, u64::MAX] {
        assert_eq!(
            Input::<u64>::from_value(json!(number)).copied(),
            Some(number)
        );
    }
    for text in [" ".to_owned(), "x".repeat(65536), "é".repeat(32768)] {
        assert_eq!(
            Input::Valid(text.clone())
                .required(InputContext::Send, "/body")
                .unwrap(),
            text
        );
    }
    for text in [String::new(), "x".repeat(65537), "é".repeat(32769)] {
        assert!(matches!(
            Input::Valid(text).required(InputContext::Send, "/body"),
            Err(Error::Input { .. })
        ));
    }
}

#[test]
fn malformed_and_unknown_operations_remain_deferred() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    for raw in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"op":false}),
        json!({"op":""}),
        json!({"op":"x".repeat(65537)}),
    ] {
        assert!(matches!(
            RequestEnvelope::new(raw.clone()).request,
            Request::Invalid
        ));
        assert_error(call(&mut core, "human", "invalid", raw), invalid());
    }
    for op in ["unknown", "invalid", "unsupported", " ", &"x".repeat(65536)] {
        assert!(matches!(
            RequestEnvelope::new(json!({"op":op})).request,
            Request::Unsupported
        ));
        assert_error(
            call(&mut core, "human", "unknown", json!({"op":op})),
            Error::unsupported_operation("Operation is not implemented in this runtime increment."),
        );
    }
    let mut terminal = agent("terminal");
    terminal.task_outcome = TaskOutcome::Completed;
    core.store().save(&terminal).unwrap();
    assert_error(
        call(&mut core, "terminal", "invalid", json!({})),
        completed(),
    );
    assert_error(
        call(&mut core, "terminal", "unknown", json!({"op":"unknown"})),
        completed(),
    );
    assert!(call(&mut core, "terminal", "read-only", json!({"op":"agents"})).is_ok());
}

#[test]
fn pagination_and_optional_control_fields_are_strict() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("recipient")).unwrap();
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "send",
            json!({"op":"send","target":"recipient","body":"work","mode":42}),
        ),
        "invalid_value",
        "/mode",
        "send",
    );
    assert_error(
        call(
            &mut core,
            "human",
            "bad-mode",
            json!({"op":"send","mode":"unknown"}),
        ),
        Error::unsupported_operation("Delivery mode must be next-turn, steer, or redirect."),
    );
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "bad-fields",
            json!({"op":"send","mode":42}),
        ),
        "invalid_value",
        "/mode",
        "send",
    );
    for raw in [Value::Null, json!(-1), json!(1.0), json!("2"), json!(false)] {
        for request in [
            json!({"op":"agents","after":raw,"limit":raw,"parent":42}),
            json!({"op":"messages","target":"recipient","before":raw,"limit":raw}),
        ] {
            let error = call(&mut core, "human", "bad-page", request).unwrap_err();
            assert_eq!(error.code(), "invalid_value");
            assert!(serde_json::to_value(error).unwrap()["field"].is_string());
        }
    }
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "agents",
            json!({"op":"agents","parent":42}),
        ),
        "invalid_value",
        "/parent",
        "agents",
    );
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "status",
            json!({"op":"status","target":"recipient","instructions":"true"}),
        ),
        "invalid_value",
        "/instructions",
        "status",
    );
    let full = call(
        &mut core,
        "human",
        "status",
        json!({"op":"status","target":"recipient","instructions":true}),
    )
    .unwrap();
    assert_eq!(full["agent"]["instructions"], "original instructions");
}

#[test]
fn stalled_arguments_reject_bad_durations_cursors_and_present_filters() {
    for raw in [
        json!({"stalled_seconds":0}),
        json!({"stalled_seconds":-1}),
        json!({"stalled_seconds":"1"}),
        json!({"stalled_seconds":null}),
        json!({"stalled_seconds":u64::MAX}),
        json!({"stalled_seconds":1,"after":0}),
        json!({"stalled_seconds":1,"parent":null}),
        json!({"stalled_seconds":1,"parent":5}),
        json!({"stalled_seconds":1,"auth":"main"}),
        json!({"stalled_seconds":1,"limit":0}),
        json!({"stalled_seconds":1,"limit":"100"}),
        json!({"stalled_seconds":1,"stalled_after":null}),
        json!({"stalled_after":{"idle_since":1,"root_id":"a"}}),
        json!({"stalled_seconds":1,"stalled_after":{"idle_since":1,"root_id":""}}),
        json!({"stalled_seconds":1,"stalled_after":{"idle_since":1,"root_id":"a","extra":1}}),
        json!({"stalled_seconds":1,"stalled_after":{"idle_since":u64::MAX,"root_id":"a"}}),
    ] {
        let args: AgentsArgs = serde_json::from_value(raw.clone()).unwrap();
        assert!(args.stalled_requested());
        assert!(args.validate_stalled().is_err(), "{raw}");
    }
    let valid = json!({"stalled_seconds":1800,"stalled_after":{"idle_since":1,"root_id":"a"}});
    validate_tool("hive_agents", &valid).unwrap();
    assert_eq!(
        serde_json::from_value::<AgentsArgs>(valid)
            .unwrap()
            .validate_stalled()
            .unwrap(),
        1800
    );
    for cursor in [
        json!("1:a"),
        json!({"idle_since":1,"root_id":"a","extra":true}),
        json!({"idle_since":0,"root_id":""}),
    ] {
        assert!(
            validate_tool(
                "hive_agents",
                &json!({"stalled_seconds":1,"stalled_after":cursor})
            )
            .is_err()
        );
    }
}
