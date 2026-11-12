//! Embedded dependencies for native mocks copied outside the source checkout.

use std::path::Path;

const SOURCES: &[(&str, &str)] = &[
    (
        "mock_codex_replies.py",
        include_str!("../fixtures/mock_codex_replies.py"),
    ),
    (
        "mock_codex_account.py",
        include_str!("../fixtures/mock_codex_account.py"),
    ),
    (
        "mock_codex_bodies.py",
        include_str!("../fixtures/mock_codex_bodies.py"),
    ),
    (
        "mock_codex_conversation.py",
        include_str!("../fixtures/mock_codex_conversation.py"),
    ),
    (
        "mock_codex_runtime.py",
        include_str!("../fixtures/mock_codex_runtime.py"),
    ),
    (
        "mock_codex_threads.py",
        include_str!("../fixtures/mock_codex_threads.py"),
    ),
    (
        "mock_codex_turns.py",
        include_str!("../fixtures/mock_codex_turns.py"),
    ),
    (
        "mock_claude_actions.py",
        include_str!("../fixtures/mock_claude_actions.py"),
    ),
    (
        "mock_claude_controls.py",
        include_str!("../fixtures/mock_claude_controls.py"),
    ),
    (
        "mock_claude_input.py",
        include_str!("../fixtures/mock_claude_input.py"),
    ),
    (
        "mock_claude_replies.py",
        include_str!("../fixtures/mock_claude_replies.py"),
    ),
    (
        "mock_claude_runtime.py",
        include_str!("../fixtures/mock_claude_runtime.py"),
    ),
    (
        "mock_claude_shell.py",
        include_str!("../fixtures/mock_claude_shell.py"),
    ),
    (
        "mock_claude_startup.py",
        include_str!("../fixtures/mock_claude_startup.py"),
    ),
    (
        "mock_claude_tasks.py",
        include_str!("../fixtures/mock_claude_tasks.py"),
    ),
];

pub(super) fn install(root: &Path) {
    for (name, source) in SOURCES {
        super::publish_fixture(&root.join(name), source.as_bytes());
    }
}
