//! Freeze all four approval query byte shapes across the typed selection change.
use super::*;

// The pre-S6c builder, retained solely as a byte oracle.
fn before(scoped: bool, all: bool) -> String {
    let scope = if scoped { "a.agent=?1 AND " } else { "" };
    let state = if all { "" } else { "a.state='pending' AND " };
    format!(
        "\nSELECT a.id, a.agent, g.role, a.title, a.destructive, a.state, a.action, \
            a.created_at, a.closed_at,\n    g.name\nFROM approvals a\nJOIN(\n    \
            SELECT id, json_extract(data, '$.role') AS role, json_extract(data, \
            '$.name') AS name\n    FROM agents) g\nON g.id = a.agent\nWHERE {scope} \
            {state} (a.created_at, a.id) > (?2, ?3)\nORDER BY a.created_at, \
            a.id\nLIMIT ?4\n"
    )
}

#[test]
fn typed_approval_selection_preserves_scope_state_spaces_and_parameter_slots() {
    for agent in [None, Some("agent")] {
        for (selection, all) in [(Selection::Pending, false), (Selection::All, true)] {
            assert_eq!(
                approvals(agent, selection).as_bytes(),
                before(agent.is_some(), all).as_bytes()
            );
        }
    }
}
