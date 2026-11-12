use super::*;

#[test]
fn approval_summary_byte_count_matches_canonical_text_for_nulls_escapes_and_unsigned_limits() {
    for title in [
        None,
        Some(String::new()),
        Some("é\n\"\0".into()),
        Some("x".repeat(65_536)),
    ] {
        let row = ApprovalSummary {
            id: "approval".into(),
            agent_id: "agent".into(),
            role: None,
            agent_name: Some("name".into()),
            title,
            destructive: true,
            state: ApprovalState::Answered,
            action: Some(ApprovalAction::Accept),
            created_at: 0,
            closed_at: Some(u64::MAX),
            age_seconds: u64::MAX,
        };
        assert_eq!(
            api::encoded_len(&row),
            crate::api::value(&row).to_string().len()
        );
    }
}
