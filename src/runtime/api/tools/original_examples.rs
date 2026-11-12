//! Frozen pre-S4 examples for exact output comparisons.
pub(super) fn example(name: &str) -> &'static str {
    match name {
        "hive_spawn" => {
            "{\"name\":\"hive_spawn\",\"arguments\":{\"role\":\"reviewer\",\"body\":\"Review \
                the work.\",\"request_id\":\"spawn-1\"}}"
        }
        "hive_send" => {
            "{\"name\":\"hive_send\",\"arguments\":{\"target\":\"AGENT_ID\",\"body\":\"Review \
                the fix.\",\"request_id\":\"send-1\"}}"
        }
        "hive_batch" => {
            "{\"name\":\"hive_batch\",\"arguments\":{\"operations\":[{\"op\":\"send\",\
            \"target\":\"AGENT_ID\",\"body\":\"Review the work.\",\"request_id\":\"member-1\"}\
            ],\"request_id\":\"batch-1\"}}"
        }
        "hive_set_auth" => {
            "{\"name\":\"hive_set_auth\",\"arguments\":{\"target\":\"AGENT_ID\",\"auth\":\
            \"codex-main\",\"request_id\":\"auth-1\"}}"
        }
        "hive_set_name" => {
            "{\"name\":\"hive_set_name\",\"arguments\":{\"target\":\"AGENT_ID\",\"name\":\
            \"reviewer\",\"request_id\":\"name-1\"}}"
        }
        "hive_options_catalog" => {
            r#"{"name":"hive_options_catalog","arguments":{"provider":"codex"}}"#
        }
        "hive_set_options" => {
            "{\"name\":\"hive_set_options\",\"arguments\":{\"target\":\"AGENT_ID\",\"options\":\
            {\"model\":\"sonnet\",\"effort\":\"low\"},\"request_id\":\"options-1\"}}"
        }
        "hive_ask" => {
            "{\"name\":\"hive_ask\",\"arguments\":{\"message\":\"May I publish the \
                reviewed change?\",\"request_id\":\"ask-1\"}}"
        }
        "hive_complete" => {
            "{\"name\":\"hive_complete\",\"arguments\":{\"body\":\"Implemented and \
                validated.\",\"request_id\":\"done-1\"}}"
        }
        "hive_terminate" => {
            "{\"name\":\"hive_terminate\",\"arguments\":{\"target\":\"AGENT_ID\",\
            \"request_id\":\"terminate-1\"}}"
        }
        "hive_agents" => r#"{"name":"hive_agents","arguments":{"limit":100}}"#,
        "hive_read" => {
            r#"{"name":"hive_read","arguments":{"target":"AGENT_ID","after":0,"limit":100}}"#
        }
        "hive_messages" => {
            r#"{"name":"hive_messages","arguments":{"target":"AGENT_ID","limit":100}}"#
        }
        #[cfg(feature = "comb")]
        "hive_evidence" => {
            "{\"name\":\"hive_evidence\",\"arguments\":{\"kind\":\"review\",\"summary\":\
            \"Input validation is missing.\",\"verdict\":\"findings\",\"request_id\":\
            \"evidence-1\"}}"
        }
        // Unknown tools get a safe known example, never the supplied name.
        _ => r#"{"name":"hive_status","arguments":{"target":"AGENT_ID"}}"#,
    }
}
