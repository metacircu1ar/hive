use super::super::{PENDING_LIMIT, test_support::RpcHarness};
use super::*;
use serde_json::json;
use std::{
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    process::ChildStdin,
};

const SHORT_BUDGET: Duration = Duration::from_millis(100);
const TEST_LIMIT: Duration = Duration::from_secs(2);

fn prepared_thread() -> RpcHarness {
    let mut h = RpcHarness::active(crate::launch::test_cache().unwrap());
    h.ingest(&json!({"method":"turn/completed","params":{
        "threadId":"thread","turn":{"id":"turn","status":"completed"}
    }}))
    .unwrap();
    h.rpc.prepare_retirement("thread").unwrap();
    h
}

fn queue_responses(h: &mut RpcHarness, responses: &[serde_json::Value]) {
    assert!(h.rpc.pending.try_extend(
        responses.iter().cloned().map(|raw| {
            let frame = super::super::IncomingFrame::parse(raw);
            let bytes = frame.encoded_len();
            (frame, bytes)
        }),
        PENDING_LIMIT
    ));
}

#[test]
fn prepared_unsubscribed_thread_absent_from_complete_inventory_is_quiescent() {
    let mut h = prepared_thread();
    let id = h.rpc.next_id + 1;
    queue_responses(
        &mut h,
        &[json!({"id":id,"result":{"data":[],"nextCursor":null}})],
    );
    assert!(h.rpc.quiesce().unwrap());
    let sent = h.outgoing(3);
    assert_eq!(sent[2]["method"], "thread/loaded/list");
    assert_eq!(
        sent.len(),
        3,
        "An unloaded thread must not get terminal cleanup"
    );
}

#[test]
fn prepared_thread_still_loaded_receives_terminal_cleanup() {
    let mut h = prepared_thread();
    let id = h.rpc.next_id + 1;
    queue_responses(
        &mut h,
        &[
            json!({"id":id,"result":{"data":["thread"],"nextCursor":null}}),
            json!({"id":id+1,"result":{}}),
            json!({"id":id+2,"result":{"data":[],"nextCursor":null}}),
        ],
    );
    assert!(h.rpc.quiesce().unwrap());
    let sent = h.outgoing(5);
    assert_eq!(sent[3]["method"], "thread/backgroundTerminals/clean");
    assert_eq!(sent[4]["method"], "thread/backgroundTerminals/list");
}

#[test]
fn absent_thread_without_prepared_retirement_is_never_proven_clean() {
    let mut h = prepared_thread();
    h.rpc.threads.get_mut("thread").unwrap().retirement_prepared = false;
    let id = h.rpc.next_id + 1;
    queue_responses(
        &mut h,
        &[json!({"id":id,"error":{"code":-32600,"message":"refused"}})],
    );
    assert!(!h.rpc.quiesce().unwrap());
    assert_eq!(
        h.outgoing(3)[2]["method"],
        "thread/backgroundTerminals/clean"
    );
}

#[test]
fn incomplete_inventory_cannot_prove_a_prepared_thread_is_absent() {
    let mut h = prepared_thread();
    let id = h.rpc.next_id + 1;
    queue_responses(
        &mut h,
        &[
            json!({"id":id,"result":{"data":[],"nextCursor":"more"}}),
            json!({"id":id+1,"error":{"code":-32600,"message":"refused"}}),
            json!({"id":id+2,"result":{"data":[],"nextCursor":"more"}}),
        ],
    );
    assert!(!h.rpc.quiesce().unwrap());
}

#[test]
fn prepared_thread_unloaded_between_list_and_clean_needs_a_fresh_absence_proof() {
    let mut h = prepared_thread();
    let id = h.rpc.next_id + 1;
    queue_responses(
        &mut h,
        &[
            json!({"id":id,"result":{"data":["thread"],"nextCursor":null}}),
            json!({"id":id+1,"error":{"code":-32600,"message":"refused"}}),
            json!({"id":id+2,"result":{"data":[],"nextCursor":null}}),
        ],
    );
    assert!(h.rpc.quiesce().unwrap());
    let sent = h.outgoing(5);
    assert_eq!(sent[3]["method"], "thread/backgroundTerminals/clean");
    assert_eq!(sent[4]["method"], "thread/loaded/list");
}

#[test]
fn lifecycle_activity_invalidates_prepared_retirement() {
    let mut h = prepared_thread();
    h.ingest(&json!({"method":"item/completed","params":{
        "threadId":"thread","turnId":"turn","item":{"id":"late","type":"commandExecution"}
    }}))
    .unwrap();
    assert!(!h.rpc.retirement_prepared("thread"));
    assert!(h.rpc.finish_retirement("thread").is_err());
}

#[test]
fn codex_shutdown_requires_group_absence_after_successful_leader_exit() {
    crate::test_support::run_isolated(&mut crate::test_support::rerun_isolated(
        "runtime::provider::rpc::shutdown::tests::codex_group_absence_fixture",
        true,
    ));
}

#[test]
#[ignore = "isolated fixture: adoption is process-wide"]
fn codex_group_absence_fixture() {
    #[cfg(target_os = "linux")]
    crate::os::adopt_orphans();
    let launcher = crate::launch::test_cache().unwrap();
    for (delay, expected) in [("0.2", true), ("gated", false)] {
        let dir = tempfile::tempdir().unwrap();
        let release = dir.path().join("release");
        let mut command = std::process::Command::new("/bin/sh");
        command
            .env("HIVE_TEST_RELEASE", &release)
            .env("HIVE_TEST_DELAY", delay)
            .args([
                "-c",
                r#"
                cat >/dev/null
                if [ "$HIVE_TEST_DELAY" = gated ]; then
                    (
                        attempts=0
                        while [ ! -f "$HIVE_TEST_RELEASE" ] && [ "$attempts" -lt 1000 ]; do
                            sleep 0.02
                            attempts=$((attempts + 1))
                        done
                    ) </dev/null >/dev/null 2>&1 &
                else
                    sleep "$HIVE_TEST_DELAY" </dev/null >/dev/null 2>&1 &
                fi
            "#,
            ]);
        let mut rpc = Rpc::start(&launcher, command, vec![]).unwrap();
        let report = rpc.close();
        std::fs::write(&release, b"release").unwrap();
        assert!(process::wait_group_absent(rpc.pid()).unwrap());
        assert_eq!(report.confirmed, expected);
        assert_eq!(report.method, ShutdownMethod::Eof);
        assert_eq!(report.exit_code, Some(0));
        assert!(report.reaped);
    }
}

#[test]
fn expired_call_never_mutates_request_or_thread_lifecycle() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    let result = h
        .rpc
        .call_until(outgoing::thread_start(None, None), Instant::now());
    assert_eq!(result.err().unwrap().code(), "provider_timeout");
    assert_eq!(h.rpc.next_id, 0);
    assert_eq!(h.rpc.opening, 0);
    assert!(h.rpc.requests.is_empty() && h.rpc.threads.is_empty());
    assert!(h.rpc.io_deadline.is_none());
}

#[test]
fn nested_budgets_take_minimum_and_restore_after_failure() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    let outer = Instant::now() + TEST_LIMIT;
    h.rpc
        .with_deadline(outer, |rpc| {
            rpc.with_deadline(outer + TEST_LIMIT, |rpc| {
                assert_eq!(rpc.io_deadline, Some(outer));
                Ok(())
            })?;
            let error = rpc.with_deadline(Instant::now(), |_| Ok(())).unwrap_err();
            assert_eq!(error.code(), "provider_timeout");
            assert_eq!(rpc.io_deadline, Some(outer));
            Ok(())
        })
        .unwrap();
    assert!(h.rpc.io_deadline.is_none());
}

#[test]
fn stalled_conversations_consume_one_quiescence_budget_not_one_each() {
    let mut h = RpcHarness::active(crate::launch::test_cache().unwrap());
    for i in 0..2 {
        let thread = format!("thread-{i}");
        let id = h.begin(outgoing::thread_start(None, None)).unwrap();
        h.response(&json!({"id":id,"result":{"thread":{"id":thread}}}))
            .unwrap();
        let id = h.begin(outgoing::turn_input(&thread, "")).unwrap();
        h.response(&json!({"id":id,"result":{"turn":{"id":"turn"}}}))
            .unwrap();
    }
    let start = Instant::now();
    let clean = h
        .rpc
        .with_deadline(start + SHORT_BUDGET, Rpc::quiesce)
        .unwrap();
    assert!(!clean);
    assert!(start.elapsed() < TEST_LIMIT);
    assert!(h.rpc.io_deadline.is_none());
    // Six setup frames and one interrupt; later siblings are never attempted.
    assert_eq!(h.outgoing(7).len(), 7);
}

fn block_input(h: &mut RpcHarness) -> (Option<ChildStdin>, UnixStream) {
    let (writer, reader) = UnixStream::pair().unwrap();
    let capacity: libc::c_int = 4096;
    assert_eq!(
        // SAFETY: a live socket and correctly sized initialized integer, retained by neither call.
        unsafe {
            libc::setsockopt(
                writer.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                std::ptr::from_ref(&capacity).cast(),
                libc::socklen_t::try_from(std::mem::size_of_val(&capacity)).unwrap(),
            )
        },
        0
    );
    writer.set_nonblocking(true).unwrap();
    let original = h
        .rpc
        .pipes
        .child
        .stdin
        .replace(ChildStdin::from(OwnedFd::from(writer)));
    (original, reader)
}

#[test]
fn unsupported_reply_write_inherits_its_call_deadline() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    let (original, _reader) = block_input(&mut h);
    let frame =
        super::super::IncomingFrame::parse(json!({"id":"x".repeat(128*1024),"method":"unknown"}));
    let bytes = frame.encoded_len();
    assert!(h.rpc.pending.try_push(frame, bytes, PENDING_LIMIT));
    let start = Instant::now();
    let result = h
        .rpc
        .call_until(outgoing::models(100), start + SHORT_BUDGET);
    h.rpc.pipes.child.stdin = original;
    let error = result.err().unwrap();
    assert_eq!(error.code(), "provider_timeout");
    assert_eq!(error.message(), "Provider stopped reading its input.");
    assert!(start.elapsed() < TEST_LIMIT);
    assert!(h.rpc.io_deadline.is_none());
}

#[test]
fn claude_next_flush_obeys_the_surrounding_deadline() {
    use crate::runtime::provider::claude::{Codec, SessionConfig};
    let launcher = crate::launch::test_cache().unwrap();
    let mut h = RpcHarness::new(launcher.clone());
    let dir = crate::runtime::test_directory();
    let config = std::sync::Arc::new(SessionConfig::for_test(
        uuid::Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    ));
    let mut command = std::process::Command::new("/bin/sh");
    let guard = config.configure(&mut command, &launcher).unwrap();
    h.native_codec(Codec::new(config, guard));
    let (original, _reader) = block_input(&mut h);
    h.rpc
        .queue_output(vec![json!({"reply":"x".repeat(128*1024)})])
        .unwrap();
    let start = Instant::now();
    let result = h.rpc.with_deadline(start + SHORT_BUDGET, Rpc::next);
    h.rpc.pipes.child.stdin = original;
    let error = result.err().unwrap();
    assert_eq!(error.code(), "provider_timeout");
    assert_eq!(error.message(), "Provider stopped reading its input.");
    assert!(start.elapsed() < TEST_LIMIT);
    assert!(h.rpc.io_deadline.is_none());
    assert!(h.rpc.uncertain());
}

#[test]
fn interrupted_turn_reply_write_uses_its_shorter_phase_budget() {
    let mut h = RpcHarness::active(crate::launch::test_cache().unwrap());
    let (original, _reader) = block_input(&mut h);
    let frame = json!({"id":"x".repeat(128*1024),"method":"unknown"});
    h.rpc
        .pipes
        .buffer
        .extend(serde_json::to_vec(&frame).unwrap());
    h.rpc.pipes.buffer.push(b'\n');
    let start = Instant::now();
    let result = h.rpc.with_deadline(start + TEST_LIMIT * 5, |rpc| {
        rpc.drain_turn("thread", start + SHORT_BUDGET)
    });
    h.rpc.pipes.child.stdin = original;
    let error = result.unwrap_err();
    assert_eq!(error.message(), "Provider stopped reading its input.");
    assert!(start.elapsed() < TEST_LIMIT);
    assert!(h.rpc.io_deadline.is_none());
}

#[test]
fn cleanup_resets_a_deadline_abandoned_by_unwinding() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.rpc
            .with_deadline(Instant::now() + TEST_LIMIT, |rpc| -> Result<()> {
                rpc.io_deadline = Some(Instant::now());
                panic!("abandoned deadline fixture");
            })
            .unwrap();
    }));
    assert!(panic.is_err());
    assert!(h.rpc.io_expired());
    let report = h.rpc.close();
    assert!(report.confirmed && report.quiescent && report.reaped);
    assert!(h.rpc.io_deadline.is_none());
}
