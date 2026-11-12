use super::*;
use crate::runtime::domain::{ModelUsage, NativeSessionUsage, NativeUsage, ProviderTurnStatus};

fn source(at: u64, outcome: ProviderTurnStatus) -> NativeSessionUsage {
    NativeSessionUsage {
        agent_id: "historical-agent".into(),
        usage: NativeUsage::new(
            crate::runtime::domain::NativeObservation {
                session_id: "session".into(),
                turn_id: "turn".into(),
                outcome,
                models: ModelUsage::default(),
                observed_at: at,
            },
            None,
        ),
    }
}
#[test]
fn native_observation_never_refreshes_account_quota_and_errors_cannot_replace_success() {
    let usages = Usages::default();
    usages.claude(
        "login",
        generation(1),
        &json!({"unifiedWindows":{"five_hour":{"utilization":0.1}}}),
        NOW,
    );
    usages.native(
        "login",
        generation(1),
        source(NOW + 10, ProviderTurnStatus::Completed),
        1,
    );
    usages.native(
        "login",
        generation(1),
        source(NOW + 20, ProviderTurnStatus::Failed),
        2,
    );
    let reports = usages.reports(Some("login"));
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].observed_at, NOW);
    assert_eq!(
        reports[0].windows[0].used_percent.to_bits(),
        10.0_f64.to_bits()
    );
    assert_eq!(
        reports[0]
            .native_session
            .as_ref()
            .unwrap()
            .usage
            .observed_at,
        NOW + 10
    );
    usages.native(
        "login",
        generation(2),
        source(NOW + 30, ProviderTurnStatus::Completed),
        3,
    );
    let reports = usages.reports(Some("login"));
    assert_eq!(reports[0].generation, generation(2).to_string());
    assert_eq!(reports[0].observed_at, 0);
    assert!(reports[0].windows.is_empty());
}

#[test]
fn account_publication_uses_committed_order_even_in_the_same_second() {
    let usages = Usages::default();
    let mut older = source(NOW, ProviderTurnStatus::Completed);
    older.agent_id = "older".into();
    let mut newer = older.clone();
    newer.agent_id = "newer".into();
    usages.native("login", generation(1), newer.clone(), 20);
    usages.native("login", generation(1), older.clone(), 19);
    usages.native("login", generation(1), older, 20);
    assert_eq!(usages.reports(None)[0].native_session, Some(newer));
}
