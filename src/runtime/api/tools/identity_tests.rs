use super::*;

#[test]
fn advertised_names_roundtrip_and_every_example_keeps_exact_bytes() {
    for name in Tool::VALUES {
        let tool = Tool::parse(name).unwrap();
        assert_eq!(tool.as_str(), *name);
        assert!(TOOLS.arguments.contains_key(&tool));
        assert_eq!(catalog::example(name), original_examples::example(name));
    }
    assert_eq!(TOOLS.arguments.len(), Tool::VALUES.len());
    assert_eq!(
        catalog::example("unknown"),
        original_examples::example("unknown")
    );
}

#[test]
fn unknown_tool_diagnostics_keep_priority_safe_examples_and_feature_boundaries() {
    for name in [
        "HIVE_STATUS",
        "Hive_agents",
        "hive_status ",
        "hive_",
        "hive_purge",
        "unknown",
        #[cfg(not(feature = "comb"))]
        "hive_evidence",
    ] {
        assert!(Tool::parse(name).is_none());
        let raw = crate::api::value(validate_tool(name, &Value::Null).unwrap_err());
        assert_eq!(
            raw,
            json!({
                "code":"unknown_argument",
                "message":"Invalid tool arguments.",
                "field":"/name",
                "expected":"an advertised Hive coordination tool",
                "examples":[original_examples::example("unknown")]
            })
        );
        assert_eq!(catalog::example(name), original_examples::example(name));
    }
}

#[test]
fn only_the_original_agent_listing_has_the_native_parallel_attribute() {
    for name in Tool::VALUES {
        assert_eq!(native_parallel_read(name), *name == "hive_agents");
    }
    for name in [
        "comb_get",
        "comb_search",
        "unknown",
        "HIVE_AGENTS",
        "hive_agents ",
    ] {
        assert!(!native_parallel_read(name));
    }
}

#[test]
fn unknown_tool_private_sentinel_never_reaches_the_fixed_diagnostic() {
    let sentinel = "hive_private_sentinel_7a95";
    let raw = crate::api::value(validate_tool(sentinel, &Value::Null).unwrap_err());
    assert!(!raw.to_string().contains(sentinel));
}
