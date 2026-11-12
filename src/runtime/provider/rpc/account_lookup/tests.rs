use super::*;
use crate::runtime::provider::{RpcHarness, protocol::IncomingFrame};
use serde_json::{Value, json};

const TIMEOUT_MESSAGE: &str = "workspace routing discovery timed out";
const SHORT_BUDGET: Duration = Duration::from_millis(50);
const RETRY_BUDGET: Duration = Duration::from_millis(400);

fn queue(harness: &mut RpcHarness, frames: &[Value]) {
    for raw in frames {
        let frame = IncomingFrame::parse(raw.clone());
        let bytes = frame.encoded_len();
        harness.rpc.pending.push_back(frame, bytes);
    }
}

fn timeout(id: u64) -> Value {
    json!({"id":id,"error":{"code":-32603,"message":TIMEOUT_MESSAGE}})
}

#[test]
fn exact_timeout_retries_only_account_lookup_once() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(
        &mut h,
        &[
            timeout(1),
            json!({"id":2,"result":{"account":{"type":"chatgpt"},"requiresOpenaiAuth":true}}),
        ],
    );
    h.rpc.validate_account().unwrap();
    let sent = h.outgoing(2);
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|row| row["method"] == "account/read"));
    assert_ne!(sent[0]["id"], sent[1]["id"]);
}

#[test]
fn repeated_timeout_stops_after_the_second_rejection() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(&mut h, &[timeout(1), timeout(2)]);
    let error = h.rpc.read_account().err().unwrap();
    assert_eq!(error.code(), "provider_request_failed");
    assert!(error.source().unwrap().is::<RoutingTimeout>());
    assert_eq!(h.outgoing(2).len(), 2);
}

#[test]
fn authentication_and_unknown_rejections_are_not_retried() {
    for (code, message) in [
        (-32603, "workspace routing discovery unauthorized (401)"),
        (-32603, "unknown internal error"),
        (-32600, TIMEOUT_MESSAGE),
    ] {
        let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
        queue(
            &mut h,
            &[json!({"id":1,"error":{"code":code,"message":message}})],
        );
        let error = h.rpc.read_account().err().unwrap();
        assert_eq!(error.code(), "provider_request_failed");
        assert!(error.source().is_none());
        assert_eq!(h.outgoing(1).len(), 1);
    }
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(
        &mut h,
        &[json!({"id":1,"result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}})],
    );
    assert_eq!(
        h.rpc.validate_account().unwrap_err().code(),
        "authentication_required"
    );
    assert_eq!(h.outgoing(1).len(), 1);
}

#[test]
fn malformed_unanswered_or_uncertain_transport_never_retries() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(&mut h, &[json!({"id":1})]);
    assert_eq!(
        h.rpc.read_account().err().unwrap().code(),
        "provider_protocol_error"
    );
    assert_eq!(h.outgoing(1).len(), 1);

    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    let result = h.rpc.read_account_until(Instant::now() + SHORT_BUDGET);
    assert_eq!(result.err().unwrap().code(), "provider_timeout");
    assert!(h.outstanding(1));
    assert_eq!(h.outgoing(1).len(), 1);

    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    h.rpc.poison();
    queue(&mut h, &[timeout(1)]);
    assert!(h.rpc.read_account().is_err());
    assert_eq!(h.outgoing(1).len(), 1);
}

#[test]
fn retries_share_the_original_deadline() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(&mut h, &[timeout(1)]);
    let began = Instant::now();
    let result = h.rpc.read_account_until(began + RETRY_BUDGET);
    assert_eq!(result.err().unwrap().code(), "provider_timeout");
    assert!(began.elapsed() < Duration::from_secs(2));
    assert!(h.outstanding(2));
    assert_eq!(h.outgoing(2).len(), 2);
}

#[test]
fn other_methods_do_not_get_the_typed_account_timeout_or_retry() {
    let mut h = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(&mut h, &[timeout(1)]);
    let error = h.rpc.call(outgoing::initialize(false)).err().unwrap();
    assert_eq!(error.code(), "provider_request_failed");
    assert!(error.source().is_none());
    assert_eq!(h.outgoing(1).len(), 1);
}
