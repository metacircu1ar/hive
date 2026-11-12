use super::*;
use crate::runtime::provider::claude::tests::Fixture;

fn fixture() -> Fixture {
    let mut f = Fixture::with_id(
        Some(Permit::Verified(ReceiptMode::NativeEcho)),
        uuid::Uuid::new_v4().to_string(),
    );
    f.turn();
    f.codec.echoes.pending.clear();
    f
}
fn guidance(text: &str) -> Value {
    super::super::super::wire::control::reply(
        "new",
        emitted::Guidance {
            hook_specific_output: emitted::Context {
                additional_context: text.into(),
            },
        },
    )
}
fn seed_reply(codec: &mut Codec, charge: usize) {
    let old =
        super::super::super::wire::control::reply("old", super::super::super::wire::mcp::Empty {});
    // Retain the boundary charge without allocating a 128 MiB old payload.
    codec.echoes.pending.push_back(old, charge);
}

#[test]
fn full_encoded_reply_budget_accepts_the_limit_and_rejects_one_more_byte() {
    let reply = guidance(&"é\n\"\0".repeat(128));
    let len = reply.to_string().len();
    let base = REPLY_ECHO_BYTES - len;
    for (index, charge) in [base - 1, base, base + 1].into_iter().enumerate() {
        let mut f = fixture();
        seed_reply(&mut f.codec, charge);
        let result = f.codec.remember_replies(std::slice::from_ref(&reply));
        if index < 2 {
            result.unwrap();
            assert_eq!(f.codec.echoes.pending.bytes(), charge + len);
            assert_eq!(f.codec.echoes.pending.len(), 2);
        } else {
            assert_eq!(
                result.unwrap_err().kind(),
                crate::error::ErrorCode::OutputLimit
            );
            assert_eq!(f.codec.echoes.pending.bytes(), charge);
            assert_eq!(f.codec.echoes.pending.len(), 1);
        }
    }
}

#[test]
fn typed_guidance_is_admitted_by_full_encoded_reply_size_before_a_receipt_is_held() {
    let text = "é\n\"\0".repeat(128);
    let reply = guidance(&text);
    let base = REPLY_ECHO_BYTES - reply.to_string().len();
    for (index, charge) in [base - 1, base, base + 1].into_iter().enumerate() {
        let mut f = fixture();
        seed_reply(&mut f.codec, charge);
        let turn = f.codec.turn.active.as_ref().unwrap().id.clone();
        f.codec.steering.held.push_back(Held {
            request: 10,
            turn,
            text: text.clone(),
            bytes: crate::api::value(&text).to_string().len() + GUIDANCE_SEPARATOR_BYTES,
        });
        let typed = f.codec.hook_guidance().unwrap();
        let mut actual = super::super::super::wire::control::reply("new", typed);
        assert_eq!(actual.to_string(), reply.to_string());
        f.codec.emitted_guidance(&mut actual).unwrap();
        assert!(f.codec.steering.held.is_empty());
        if index < 2 {
            assert_eq!(actual.to_string(), reply.to_string());
            assert_eq!(f.codec.steering.emitted.len(), 1);
            assert!(f.codec.pop().is_none());
        } else {
            assert!(!super::super::super::wire::Decision::read(&actual).has_guidance());
            assert!(f.codec.steering.emitted.is_empty());
            assert!(f.codec.pop().is_some());
        }
        f.codec.remember_replies(&[actual]).unwrap();
    }
}
