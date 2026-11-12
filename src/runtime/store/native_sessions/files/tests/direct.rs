use super::*;

#[test]
fn unrelated_provider_trees_are_not_traversed() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    write(&home, &rollout(SESSION));
    let unrelated = directory(&home, Path::new("sessions/1900"));
    symlink(temp.path(), unrelated.join("do-not-walk")).unwrap();
    let projects = directory(&home, Path::new("projects"));
    symlink(temp.path(), projects.join("unrelated-project")).unwrap();
    for i in 0..100 {
        directory(&home, &Path::new("sessions").join(format!("unrelated-{i}")));
    }
    DIRECTORY_VISITS.set(0);
    let selected = plan(&home, Provider::Codex, SESSION).unwrap();
    assert_eq!(selected.warning, None);
    assert_eq!(selected.files.len(), 1);
    assert_eq!(DIRECTORY_VISITS.get(), 0);
    write(&home, &format!("projects/project/{SESSION}.jsonl"));
    let selected = plan(&home, Provider::Claude, SESSION).unwrap();
    assert_eq!(selected.warning, None);
    assert_eq!(selected.files.len(), 1);
    assert_eq!(DIRECTORY_VISITS.get(), 0);
}

#[test]
fn codex_archive_uses_the_recorded_basename_without_a_date_tree_walk() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let archived = write(
        &home,
        &format!("archived_sessions/rollout-2026-11-05-{SESSION}.jsonl"),
    );
    let selected = plan(&home, Provider::Codex, SESSION).unwrap();
    assert_eq!(selected.warning, None);
    assert_eq!(selected.files.len(), 1);
    assert_eq!(home.join(&selected.files[0].path), archived);
    assert_eq!(
        plan_at(&home, Provider::Codex, SESSION, &[])
            .unwrap()
            .warning,
        Some(Warning::IncompleteHistory)
    );
}

#[test]
fn unverified_claude_location_is_never_reported_as_absent() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let expected = format!("projects/project/{SESSION}.jsonl");
    let actual = write(
        &home,
        &format!("projects/changed-native-encoding/{SESSION}.jsonl"),
    );
    let selected = plan_at(&home, Provider::Claude, SESSION, &[(&expected, false)]).unwrap();
    assert_eq!(selected.warning, Some(Warning::IncompleteHistory));
    assert!(selected.files.is_empty());
    assert!(actual.exists());
    write(&home, &expected);
    let selected = plan_at(&home, Provider::Claude, SESSION, &[(&expected, false)]).unwrap();
    assert_eq!(selected.warning, None);
    assert_eq!(selected.files.len(), 1);
}

#[test]
fn claude_resume_cleans_transcript_and_current_project_tool_results() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let transcript = format!("projects/original/{SESSION}.jsonl");
    let current = format!("projects/current/{SESSION}.jsonl");
    let files = [
        write(&home, &transcript),
        write(
            &home,
            &format!("projects/original/{SESSION}/subagents/agent-x.jsonl"),
        ),
        write(
            &home,
            &format!("projects/current/{SESSION}/tool-results/output.txt"),
        ),
    ];
    let selected = plan_at(
        &home,
        Provider::Claude,
        SESSION,
        &[(&transcript, true), (&current, false), (&current, false)],
    )
    .unwrap();
    assert_eq!(selected.warning, None);
    assert_eq!(selected.files.len(), 3);
    for file in selected.files {
        unlink(&home, Provider::Claude, SESSION, &file).unwrap();
    }
    assert!(files.iter().all(|path| !path.exists()));
}

#[test]
fn todos_are_exact_names_from_direct_child_transcripts_without_shared_enumeration() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    write(&home, &format!("projects/project/{SESSION}.jsonl"));
    let owned = ["worker-a", "worker_b"];
    for id in owned {
        write(
            &home,
            &format!("projects/project/{SESSION}/subagents/agent-{id}.jsonl"),
        );
        write(&home, &format!("todos/{SESSION}-agent-{id}.json"));
    }
    write(&home, &format!("todos/{SESSION}-agent-{SESSION}.json"));
    let unrelated = write(&home, &format!("todos/{SESSION}-agent-unproved.json"));
    // Neither this directory nor an unrelated symlink is enumerated.
    symlink(temp.path(), home.join("todos/unrelated")).unwrap();
    write(
        &home,
        &format!("projects/project/{SESSION}/subagents/nested/agent-hidden.jsonl"),
    );
    let nested = write(&home, &format!("todos/{SESSION}-agent-hidden.json"));
    DIRECTORY_VISITS.set(0);
    let selected = plan(&home, Provider::Claude, SESSION).unwrap();
    assert_eq!(selected.warning, None);
    assert_eq!(DIRECTORY_VISITS.get(), 3); // session, subagents, nested
    assert_eq!(selected.files.len(), 7);
    for file in selected.files {
        unlink(&home, Provider::Claude, SESSION, &file).unwrap();
    }
    assert!(unrelated.exists());
    assert!(nested.exists());
}

#[test]
fn exact_todo_symlinks_and_unsafe_child_ids_retain_the_session() {
    for id in ["worker", "..", ""] {
        let temp = crate::runtime::test_directory();
        let home = home(temp.path(), uuid::Uuid::new_v4());
        write(&home, &format!("projects/project/{SESSION}.jsonl"));
        write(
            &home,
            &format!("projects/project/{SESSION}/subagents/agent-{id}.jsonl"),
        );
        if id == "worker" {
            directory(&home, Path::new("todos"));
            symlink(
                temp.path(),
                home.join(format!("todos/{SESSION}-agent-worker.json")),
            )
            .unwrap();
        }
        let selected = plan(&home, Provider::Claude, SESSION).unwrap();
        assert_eq!(selected.warning, Some(Warning::UnsafePath));
        assert!(selected.files.is_empty());
    }
}

#[test]
fn derived_todo_basename_limit_retains_unsupported_names_before_lookup() {
    for length in [207, 208] {
        let temp = crate::runtime::test_directory();
        let home = home(temp.path(), uuid::Uuid::new_v4());
        write(&home, &format!("projects/project/{SESSION}.jsonl"));
        let id = "a".repeat(length);
        write(
            &home,
            &format!("projects/project/{SESSION}/subagents/agent-{id}.jsonl"),
        );
        directory(&home, Path::new("todos"));
        let name = format!("todos/{SESSION}-agent-{id}.json");
        if length == 207 {
            write(&home, &name);
        }
        let selected = plan(&home, Provider::Claude, SESSION).unwrap();
        if length == 207 {
            assert_eq!(selected.warning, None);
            assert_eq!(selected.files.len(), 3);
            assert!(
                selected
                    .files
                    .iter()
                    .any(|file| file.path == Path::new(&name))
            );
        } else {
            assert_eq!(selected.warning, Some(Warning::UnsafePath));
            assert!(selected.files.is_empty());
        }
    }
}

#[test]
fn exact_todo_inspections_charge_the_owned_work_budget() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path(), uuid::Uuid::new_v4());
    let transcript = format!("projects/project/{SESSION}.jsonl");
    for name in [
        &transcript,
        &format!("projects/project/{SESSION}/subagents/agent-worker.jsonl"),
        &format!("todos/{SESSION}-agent-{SESSION}.json"),
        &format!("todos/{SESSION}-agent-worker.json"),
    ] {
        write(&home, name);
    }
    let locations = [super::super::super::locations::Location {
        path: transcript,
        verified: true,
    }];
    for work in [5, 6, 7] {
        let mut budget = Budget::new(Some(work));
        let selected = discover(&home, Provider::Claude, SESSION, &locations, &mut budget).unwrap();
        if work < 7 {
            assert!(selected.is_none());
            assert!(budget.exhausted);
        } else {
            let selected = selected.unwrap();
            assert_eq!(selected.warning, None);
            assert_eq!(selected.files.len(), 4);
            assert!(!budget.exhausted);
        }
    }
}
