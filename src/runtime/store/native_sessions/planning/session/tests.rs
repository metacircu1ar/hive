//! Synthetic metadata below is used only for preview, never as unlink authority.
use super::*;

fn binding() -> Binding {
    Binding {
        agent: "agent".into(),
        generation: uuid::Uuid::nil().to_string(),
        provider: "codex".into(),
        thread: "thread".into(),
    }
}
fn file(index: usize) -> files::File {
    serde_json::from_value(serde_json::json!({"path":format!("file-{index}"),
        "identity":{"device":0,"inode":0,"bytes":3,"modified_seconds":1,"modified_nanos":0}}))
    .unwrap()
}

#[test]
fn preview_retains_full_byte_totals_and_the_existing_file_cap() {
    for count in [
        0,
        api::NATIVE_PREVIEW_LIMIT - 1,
        api::NATIVE_PREVIEW_LIMIT,
        api::NATIVE_PREVIEW_LIMIT + 1,
    ] {
        let files: Vec<_> = (0..count).map(file).collect();
        let Preview::Entry(entry) = preview(
            &binding(),
            Provider::Codex,
            uuid::Uuid::nil(),
            &files,
            false,
        ) else {
            panic!("preview entry")
        };
        assert_eq!(entry.agent_id, "agent");
        assert_eq!(entry.provider, Provider::Codex);
        assert_eq!(entry.generation, uuid::Uuid::nil());
        assert_eq!(entry.thread_id, "thread");
        assert_eq!(entry.files.len(), count.min(api::NATIVE_PREVIEW_LIMIT));
        assert_eq!(entry.bytes, u64::try_from(count).unwrap() * 3);
        assert_eq!(entry.files_truncated, count > api::NATIVE_PREVIEW_LIMIT);
    }
}

#[test]
fn each_outcome_is_applied_before_the_next_session_checks_report_capacity() {
    let mut report = api::NativeCleanup::new(false);
    for _ in 0..=api::NATIVE_PREVIEW_LIMIT {
        let full = report.native_preview.len() >= api::NATIVE_PREVIEW_LIMIT;
        let preview = preview(&binding(), Provider::Codex, uuid::Uuid::nil(), &[], full);
        report.record_session(
            "agent",
            Outcome {
                disposition: Disposition::Retained,
                warning: None,
                preview,
            },
        );
    }
    assert_eq!(report.native_preview.len(), api::NATIVE_PREVIEW_LIMIT);
    assert!(report.native_preview_truncated);
    assert_eq!(
        report.native_sessions_retained,
        u64::try_from(api::NATIVE_PREVIEW_LIMIT + 1).unwrap()
    );
    assert!(
        report
            .native_preview
            .iter()
            .all(|entry| entry.files.is_empty() && entry.bytes == 0)
    );
}

#[test]
fn retained_absent_queued_and_warning_outcomes_keep_separate_counts() {
    let mut report = api::NativeCleanup::new(false);
    report.record_session("agent", Outcome::retained(None));
    report.record_session(
        "agent",
        Outcome::retained(Some(api::NativeWarningReason::SharedSession)),
    );
    for disposition in [Disposition::Absent, Disposition::Queued] {
        report.record_session(
            "agent",
            Outcome {
                disposition,
                warning: None,
                preview: Preview::NotRequested,
            },
        );
    }
    assert_eq!(report.native_sessions_retained, 2);
    assert_eq!(report.native_sessions_absent, 1);
    assert_eq!(report.native_sessions_deleted, 0);
    assert_eq!(report.native_bytes_deleted, 0);
    assert_eq!(report.native_history, api::NativeHistory::Partial);
    assert_eq!(report.native_warning_count, 1);
    assert_eq!(report.native_warnings.len(), 1);
    for _ in 0..api::NATIVE_PREVIEW_LIMIT {
        report.record_session(
            "agent",
            Outcome::retained(Some(api::NativeWarningReason::SharedSession)),
        );
    }
    assert_eq!(report.native_warnings.len(), api::NATIVE_PREVIEW_LIMIT);
    assert_eq!(
        report.native_warning_count,
        u64::try_from(api::NATIVE_PREVIEW_LIMIT + 1).unwrap()
    );
}
