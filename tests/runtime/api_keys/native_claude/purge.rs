//! Actual native child/tool-output files remain until the final branch is purged.
use super::*;
use std::collections::BTreeSet;

struct Files {
    paths: BTreeSet<PathBuf>,
    children: usize,
    results: usize,
    todos: usize,
}

fn walk(path: &Path, files: &mut BTreeSet<PathBuf>, depth: usize, visited: &mut usize) {
    *visited += 1;
    assert!(
        depth <= 12 && *visited <= 1_000,
        "native fixture work budget"
    );
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("native fixture metadata: {error}"),
    };
    assert!(!metadata.file_type().is_symlink());
    if metadata.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            walk(&entry.unwrap().path(), files, depth + 1, visited);
        }
    } else {
        assert!(metadata.is_file());
        files.insert(path.to_path_buf());
    }
}

impl Files {
    fn capture(f: &NativeFixture, cwd: &Path, alternate: &Path, session: &str) -> Self {
        let mut paths = BTreeSet::new();
        let mut visited = 0;
        for transcript in
            BTreeSet::from([f.transcript(cwd, session), f.transcript(alternate, session)])
        {
            walk(&transcript, &mut paths, 0, &mut visited);
            walk(
                &transcript.parent().unwrap().join(session),
                &mut paths,
                0,
                &mut visited,
            );
        }
        let children: Vec<_> = paths
            .iter()
            .filter(|path| {
                path.parent().unwrap().file_name().unwrap() == "subagents"
                    && path.extension().is_some_and(|ext| ext == "jsonl")
            })
            .map(|path| {
                path.file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .strip_prefix("agent-")
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let results = paths
            .iter()
            .filter(|path| path.parent().unwrap().file_name().unwrap() == "tool-results")
            .count();
        let mut todos = 0;
        for id in std::iter::once(session).chain(children.iter().map(String::as_str)) {
            assert!(
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            );
            let path = f
                .provider_home
                .join("todos")
                .join(format!("{session}-agent-{id}.json"));
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
                    paths.insert(path);
                    todos += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("native fixture todo: {error}"),
            }
        }
        Self {
            paths,
            children: children.len(),
            results,
            todos,
        }
    }
}

#[test]
#[ignore = "requires HIVE_NATIVE_CLAUDE; real CLI, isolated scripted HTTP, no account/model"]
fn source_purge_retains_native_files_until_the_last_branch_is_purged() {
    let mut f = NativeFixture::new("purge");
    f.hive.start();
    let source = f.hive.ok(
        json!({"op":"spawn","role":"native","cwd":f.hive.tmp.path(),"body":"NATIVE_ROOT_PARALLEL"}),
    );
    let source_id = source["agent_id"].as_str().unwrap();
    super::super::super::claude::answered(&f.hive, source_id, "NATIVE_ROOT_DONE");
    f.assert_parent_output(source_id, "NATIVE_ROOT_DONE");
    let original = f.hive.settled(source_id);
    let cwd = f.hive.tmp.path().to_owned();
    let session = original.thread_id.as_deref().unwrap();
    let owned = Files::capture(&f, &cwd, &cwd, session);
    assert!(
        owned.children >= 2,
        "actual native child transcripts required"
    );
    assert!(
        owned.results > 0,
        "actual native persisted tool output required"
    );
    let branch_cwd = cwd.join("branch-workspace");
    fs::create_dir(&branch_cwd).unwrap();
    let branch = f.hive.ok(
        json!({"op":"spawn","from_agent":source_id,"cwd":branch_cwd,"body":"NATIVE_ROOT_BRANCH"}),
    );
    let id = branch["agent_id"].as_str().unwrap();
    super::super::super::claude::answered(&f.hive, id, "NATIVE_BRANCH_DONE");
    f.assert_parent_output(id, "NATIVE_BRANCH_DONE");
    f.hive.ok(json!({"op":"terminate","target":source_id}));
    f.hive.wait_purgeable(source_id);
    let first = f.hive.ok(json!({"op":"purge","target":source_id}));
    assert_eq!(first["native_sessions_retained"], 1, "{first}");
    assert!(owned.paths.iter().all(|path| path.is_file()));
    f.hive.ok(json!({"op":"stop","target":id}));
    f.hive.wait(id, |agent| agent.state == "stopped");
    f.hive
        .ok(json!({"op":"send","target":id,"body":"NATIVE_ROOT_RESUME"}));
    f.hive.ok(json!({"op":"resume","target":id}));
    super::super::super::claude::answered(&f.hive, id, "NATIVE_RESUME_DONE");
    f.assert_parent_output(id, "NATIVE_RESUME_DONE");
    let requests = NativeFixture::rows(&f.receipts);
    for role in ["NATIVE_ROOT_BRANCH", "NATIVE_ROOT_RESUME"] {
        assert!(
            requests
                .iter()
                .any(|row| row["role"] == role && row["inherited_root"] == true)
        );
    }
    let resumed = f.hive.settled(id);
    let child_session = resumed.thread_id.as_deref().unwrap();
    assert_ne!(child_session, session);
    let child_files = Files::capture(&f, &branch_cwd, &cwd, child_session);
    assert!(!child_files.paths.is_empty());
    f.hive.ok(json!({"op":"terminate","target":id}));
    f.hive.wait_purgeable(id);
    let last = f.hive.ok(json!({"op":"purge","target":id}));
    assert_eq!(last["native_sessions_deleted"], 2, "{last}");
    for path in owned.paths.iter().chain(&child_files.paths) {
        assert!(
            !path.exists(),
            "proved native file retained: {}",
            path.display()
        );
    }
    f.cleanup = Some(
        json!({"source_files":owned.paths.len(), "source_child_transcripts":owned.children,
        "source_tool_results":owned.results, "source_derived_todos":owned.todos,
        "branch_files":child_files.paths.len(), "branch_derived_todos":child_files.todos,
        "retained_while_shared":true, "removed_after_final_owner":true}),
    );
    f.hive.shutdown();
    f.assert_capture();
    f.export().unwrap();
}
