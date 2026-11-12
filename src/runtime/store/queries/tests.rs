//! Byte checks against the expressions frozen before S2.
use super::*;
mod original;

#[test]
fn query_bytes_match_before_wrapping() {
    assert_eq!(
        STALLED_AGENTS.as_bytes(),
        original::STALLED_AGENTS.as_bytes()
    );
    // M2c intentionally preserves persisted admission fences on restart.
    assert_ne!(
        RECOVER_AGENTS.as_bytes(),
        original::RECOVER_AGENTS.as_bytes()
    );
}
