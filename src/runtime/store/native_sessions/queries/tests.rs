//! Byte checks against the expressions frozen before S2.
use super::*;
mod original;

#[test]
fn query_bytes_match_before_wrapping() {
    for (actual, expected) in [
        (INHERIT_CLAUDE_BINDINGS, original::INHERIT_CLAUDE_BINDINGS),
        (OWNED_CLAUDE_PATHS, original::OWNED_CLAUDE_PATHS),
        (SCOPE_SESSIONS, original::SCOPE_SESSIONS),
        (AGENT_SESSIONS, original::AGENT_SESSIONS),
        (
            ADMISSION_SURVIVING_OWNER,
            original::ADMISSION_SURVIVING_OWNER,
        ),
        (INSERT_JOB, original::INSERT_JOB),
        (INSERT_SESSION, original::INSERT_SESSION),
        (PENDING_FILES, original::PENDING_FILES),
        (FILE_SESSION, original::FILE_SESSION),
        (SAVE_SESSION, original::SAVE_SESSION),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}
