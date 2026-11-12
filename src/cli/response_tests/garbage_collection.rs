//! Garbage collection regressions.
use super::*;

#[test]
fn gc_cli_error_reports_acknowledged_progress_and_failed_page_identity() {
    for dry_run in [false, true] {
        for keep_native_history in [false, true] {
            failed_page_case(crate::runtime::api::CleanupOptions {
                dry_run,
                keep_native_history,
            });
        }
    }
}
fn failed_page_case(options: crate::runtime::api::CleanupOptions) {
    let mut page = gc_page(10, false, Some(json!({"ended_at":1,"id":"a"})));
    page["dry_run"] = json!(options.dry_run);
    page["stats"]["deleted_agents"] = json!(u64::from(!options.dry_run));
    page.as_object_mut().unwrap().extend(
        crate::api::value(crate::runtime::api::NativeCleanup::new(
            options.keep_native_history || options.dry_run,
        ))
        .as_object()
        .unwrap()
        .clone(),
    );
    let stub = Stub::new(vec![
        page,
        json!({"error":{"code":"purge_cleanup_pending",
        "message":"retry"},"request_id":"daemon-retry"}),
    ]);
    let result = super::super::gc::run_before(stub.root(), 10, options);
    assert!(!result.success);
    assert_eq!(result.value["error"]["code"], "purge_cleanup_pending");
    assert_eq!(result.value["request_id"], "daemon-retry");
    let progress = &result.value["gc_progress"];
    assert_eq!(progress["acknowledged_pages"], 1);
    assert_eq!(
        progress["stats"]["deleted_agents"],
        u64::from(!options.dry_run)
    );
    assert_eq!(progress["complete"], false);
    assert_eq!(progress["dry_run"], options.dry_run);
    assert_eq!(
        progress["native_history"],
        if options.keep_native_history || options.dry_run {
            "retained"
        } else {
            "deleted"
        }
    );
    assert_eq!(
        result.value["failed_page"]["may_have_committed"],
        !options.dry_run
    );
    let requests = stub.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(result.value["failed_page"]["request_id"], requests[1]["id"]);
    let args = json!({"op":"gc","before":10,"after":{"ended_at":1,"id":"a"},
        "dry_run":options.dry_run,"keep_native_history":options.keep_native_history});
    assert_eq!(result.value["failed_page"]["args"], args);
    assert_eq!(requests[1]["args"], args);
}

#[test]
fn gc_cli_retries_lost_reply_and_cleanup_with_same_identity_and_counts_once() {
    let next = Some(json!({"ended_at":1,"id":"a"}));
    let stub = Stub::replies(vec![
        None,
        Some(gc_page(10, true, next.clone())),
        Some(gc_page(10, false, next)),
        Some(gc_page(10, false, None)),
    ]);
    let result = super::super::gc::run_before(
        stub.root(),
        10,
        crate::runtime::api::CleanupOptions::default(),
    );
    assert!(result.success);
    assert_eq!(result.value["acknowledged_pages"], 2);
    assert_eq!(result.value["stats"]["deleted_agents"], 2);
    let requests = stub.finish();
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[1], requests[2]);
    assert_ne!(requests[2]["id"], requests[3]["id"]);
    assert_eq!(requests[3]["args"]["after"], json!({"ended_at":1,"id":"a"}));
}

#[test]
fn gc_cli_accepts_the_control_sockets_request_receipt() {
    let mut page = gc_page(10, false, None);
    page["request_id"] = json!("receipt");
    let stub = Stub::new(vec![page]);
    let result = super::super::gc::run_before(
        stub.root(),
        10,
        crate::runtime::api::CleanupOptions::default(),
    );
    assert!(result.success, "{}", result.value);
    assert_eq!(result.value["stats"]["deleted_agents"], 1);
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn gc_cli_rejects_missing_unknown_and_nonobject_response_fields() {
    let good = gc_page(10, false, None);
    let mut malformed = vec![
        json!([10, false, {}, [], [], null, false]),
        json!({"error":null}),
        json!({"error":{"code":"x","message":"m"},"request_id":7}),
    ];
    for path in ["", "/stats"] {
        for key in good.pointer(path).unwrap().as_object().unwrap().keys() {
            let mut missing = good.clone();
            missing
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            malformed.push(missing);
        }
        let mut unknown = good.clone();
        unknown.pointer_mut(path).unwrap()["unexpected"] = json!(true);
        malformed.push(unknown);
    }
    for (field, value) in [
        ("request_id", json!(7)),
        ("stats", json!([1, 1, 1, 0, 2, 0, 3, 0, 0, 6, 6])),
        ("blockers", json!([["a", "busy"]])),
        ("next_after", json!([1, "a"])),
        ("native_warnings", json!([["a", "unsafe_path"]])),
        ("native_history", json!({"deleted": null})),
        (
            "native_warnings",
            json!([{"agent_id":"a", "reason":{"unsafe_path":null}}]),
        ),
        (
            "native_preview",
            json!([{"agent_id":"a","provider":{"codex":null},"generation":"00000000-0000-0000-0000-000000000000","thread_id":"thread","files":[],"bytes":0,"files_truncated":false}]),
        ),
        (
            "native_preview",
            json!([[
                "a",
                "codex",
                "00000000-0000-0000-0000-000000000000",
                "thread",
                [],
                0,
                false
            ]]),
        ),
        (
            "native_preview",
            json!([{"agent_id":"a","provider":"codex","generation":"00000000-0000-0000-0000-000000000000","thread_id":"thread","files":[["relative",1]],"bytes":1,"files_truncated":false}]),
        ),
    ] {
        let mut value_page = good.clone();
        value_page[field] = value;
        malformed.push(value_page);
    }
    for value in malformed {
        let stub = Stub::new(vec![value]);
        let result = super::super::gc::run_before(
            stub.root(),
            10,
            crate::runtime::api::CleanupOptions::default(),
        );
        assert!(!result.success);
        assert_eq!(result.value["error"]["code"], "daemon_protocol_error");
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn gc_cli_rejects_nonadvancing_cursor_instead_of_looping() {
    let next = Some(json!({"ended_at":1,"id":"a"}));
    let stub = Stub::new(vec![
        gc_page(10, false, next.clone()),
        gc_page(10, false, next),
    ]);
    let result = super::super::gc::run_before(
        stub.root(),
        10,
        crate::runtime::api::CleanupOptions::default(),
    );
    assert!(!result.success);
    assert_eq!(result.value["error"]["code"], "daemon_protocol_error");
    assert_eq!(stub.finish().len(), 2);
}
