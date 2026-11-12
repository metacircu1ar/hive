//! Byte checks against the expressions frozen before S2.
use super::*;
mod original;

#[test]
fn query_bytes_match_before_wrapping() {
    for (actual, expected) in [
        (OUTPUT, original::OUTPUT),
        (CLEAR_OUTCOME_DIRTY, original::CLEAR_OUTCOME_DIRTY),
        (DISARMED_CUTOFFS, original::DISARMED_CUTOFFS),
        (WAITING, original::WAITING),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}
