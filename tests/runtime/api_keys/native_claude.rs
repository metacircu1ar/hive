//! Opt-in real Claude CLI against a scripted loopback endpoint; no account/model.
use super::*;
use hive::config::Sandbox;
use std::path::Path;
mod branching;
mod identity;
mod observations;
mod parallel;
mod purge;
mod setup;

struct NativeFixture {
    hive: Harness,
    _endpoint: support::ChildGuard,
    trace: PathBuf,
    receipts: PathBuf,
    exported: bool,
    native_version: String,
    case: &'static str,
    usage: Vec<observations::Usage>,
    histories: Vec<observations::History>,
    provider_home: PathBuf,
    cleanup: Option<Value>,
}

impl NativeFixture {
    fn new(case: &'static str) -> Self {
        setup::fixture(case)
    }

    fn rows(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn record_usage(&mut self, status: &AgentView) {
        let usage: observations::Usage =
            serde_json::from_value(status.native_usage.as_ref().unwrap().clone()).unwrap();
        assert!(
            usage.current(
                status.thread_id.as_deref(),
                status.last_finished_turn.as_ref()
            ),
            "usage must belong to the current message-bound completed turn"
        );
        assert!(!usage.models.is_empty() && usage.models.len() <= 32);
        assert!(self.usage.len() < 4);
        self.usage.push(usage);
    }
    fn transcript(&self, cwd: &Path, session: &str) -> PathBuf {
        let cwd = cwd.canonicalize().unwrap();
        let text = cwd.to_str().unwrap();
        assert!(
            text.is_ascii() && text.len() <= 200,
            "native fixture uses short ASCII working directories"
        );
        let project: String = text
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        self.provider_home
            .join("projects")
            .join(project)
            .join(format!("{session}.jsonl"))
    }
    fn record_history(&mut self, cwd: &Path, alternative: &Path, session: &str) {
        let candidates = std::collections::BTreeSet::from([
            self.transcript(cwd, session),
            self.transcript(alternative, session),
        ]);
        let present: Vec<_> = candidates.iter().filter(|path| path.exists()).collect();
        assert_eq!(
            present.len(),
            1,
            "one native transcript in the known projects"
        );
        self.histories
            .push(observations::history(present[0], session));
    }

    fn assert_capture(&self) {
        let rows = Self::rows(&self.trace);
        let mut launches = std::collections::BTreeMap::<&str, Vec<&Value>>::new();
        for row in &rows {
            let launch = row["launch"].as_str().unwrap();
            uuid::Uuid::parse_str(launch).unwrap();
            launches.entry(launch).or_default().push(row);
        }
        assert!(!launches.is_empty(), "no native capture launches");
        for (launch, frames) in launches {
            assert_eq!(
                frames
                    .iter()
                    .filter(|row| row["kind"] == "capture_started")
                    .count(),
                1,
                "{launch}: {frames:?}"
            );
            let complete: Vec<_> = frames
                .iter()
                .filter(|row| row["kind"] == "capture_complete")
                .collect();
            assert_eq!(complete.len(), 1, "{launch}: {frames:?}");
            if complete[0]["ok"] != true {
                // Claude can exit 1 after an explicitly interrupted native turn.
                // The interrupt case separately proves the interrupted outcome,
                // Bash reaping and a successful next turn on this conversation.
                assert_eq!(self.case, "interrupt", "{launch}: {frames:?}");
                assert_eq!(complete[0]["native_status"], 1, "{launch}: {frames:?}");
                assert_eq!(
                    frames
                        .iter()
                        .filter(|row| row["kind"] == "interrupt_requested")
                        .count(),
                    1,
                    "{launch}: {frames:?}"
                );
                assert!(
                    frames.iter().any(|row| row["kind"] == "native_result"
                        && row["is_error"] == true
                        && row["subtype"] == "error_during_execution"),
                    "{launch}: {frames:?}"
                );
            }
        }
        assert!(
            !rows.iter().any(|row| row["kind"] == "transport_error"),
            "{rows:?}"
        );
        let requests = Self::rows(&self.receipts);
        assert!(
            !requests.iter().any(|row| row["kind"] == "fixture_error"),
            "{requests:?}"
        );
    }

    fn assert_parent_output(&self, id: &str, expected: &str) {
        let status = self.hive.settled(id);
        assert!(status.error.is_none(), "{status:?}");
        let finished = status.last_finished_turn.as_ref().unwrap();
        assert!(finished.outcome == "completed", "{finished:?}");
        // Startup/branch input deliberately has no external call result. Read
        // the public output stream, fenced to this exact completed turn.
        let events = self.hive.read(id);
        let output = events
            .events
            .iter()
            .filter(|event| {
                event.kind == "assistant_message"
                    && event.execution_id.as_deref() == Some(finished.execution.as_str())
                    && event.data["turn_id"] == finished.turn
            })
            .map(|event| event.data["text"].as_str().unwrap())
            .collect::<String>();
        assert!(output.contains(expected), "{output}");
        assert!(!output.contains("NATIVE_PRIVATE_DONE"), "{output}");
        let events = self.hive.events(id);
        assert!(!events.events.iter().any(|event| {
            event.kind == "assistant_message"
                && event.data["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("NATIVE_PRIVATE_DONE"))
        }));
    }

    fn export(&mut self) -> std::io::Result<()> {
        if self.exported {
            return Ok(());
        }
        if let Some(root) = std::env::var_os("HIVE_NATIVE_EVIDENCE") {
            let root = PathBuf::from(root);
            if !root.is_absolute() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Native evidence path must be absolute.",
                ));
            }
            fs::create_dir_all(&root)?;
            let root = root.join(self.case);
            fs::create_dir(&root)?;
            fs::write(
                root.join("producer.json"),
                serde_json::to_vec(&json!({
                    "native_version":self.native_version,
                    "transport":"native CLI through fixture stdio proxy",
                    "inference":"scripted loopback HTTP; no account or model",
                    "fixture_plugin":"explicit local fork-skill protocol fixture; no reposito\
                        ry/user settings",
                    "native_session_usage":self.usage,
                    "native_history":self.histories,
                    "native_cleanup":self.cleanup,
                }))
                .map_err(std::io::Error::other)?,
            )?;
            for (source, name) in [
                (&self.trace, "native-observations.jsonl"),
                (&self.receipts, "native-requests.jsonl"),
            ] {
                if source.exists() {
                    fs::copy(source, root.join(name))?;
                }
            }
        }
        self.exported = true;
        Ok(())
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        if let Err(error) = self.export() {
            // Preserve the primary assertion failure and report only bounded OS metadata.
            eprintln!("Native fixture evidence export failed: {:?}", error.kind());
        }
    }
}

#[test]
#[ignore = "requires HIVE_NATIVE_CLAUDE; real CLI with scripted local HTTP, no account or model"]
fn foreground_native_agents_use_parent_policy_and_cannot_dispatch_hosted_tools() {
    let mut f = NativeFixture::new("parallel");
    f.hive.start();
    let spawned = f.hive.ok(
        json!({"op":"spawn", "role":"native", "cwd":f.hive.tmp.path(),
        "body":"NATIVE_ROOT_PARALLEL"}),
    );
    let id = spawned["agent_id"].as_str().unwrap();
    super::super::claude::answered(&f.hive, id, "NATIVE_ROOT_DONE");
    let status = f.hive.settled(id);
    assert!(status.error.is_none(), "{status:?}");
    f.assert_parent_output(id, "NATIVE_ROOT_DONE");
    assert_eq!(
        fs::read_to_string(f.hive.tmp.path().join("native-a.txt")).unwrap(),
        "native-a"
    );
    assert_eq!(
        fs::read_to_string(f.hive.tmp.path().join("native-b.txt")).unwrap(),
        "native-b-edited"
    );
    let rows = NativeFixture::rows(&f.trace);
    let child = parallel::tasks(&rows);
    parallel::dispatch_order(&rows);
    parallel::permissions(&rows, child);
    let requests = NativeFixture::rows(&f.receipts);
    parallel::child_requests(&rows, &requests, child);
    parallel::selected_models(&requests);
    let external = requests
        .iter()
        .find(|row| row["role"] == "NATIVE_CHILD_A" && row["external_ok"] == true)
        .unwrap();
    let pid = u32::try_from(external["external_pid"].as_u64().unwrap()).unwrap();
    let mcp = hive::state::ProcessIdentity::read(pid).unwrap().unwrap();
    f.hive.ok(json!({"op":"terminate", "target":id}));
    f.hive.shutdown();
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "native external MCP process disappearance",
        || mcp.absent().unwrap().then_some(()),
    );
    f.assert_capture();
    f.export().unwrap();
}

#[test]
#[ignore = "requires HIVE_NATIVE_CLAUDE; real CLI with scripted local HTTP, no account or model"]
fn forked_native_skill_cannot_dispatch_hosted_tools() {
    verify_native_skill(Sandbox::WorkspaceWrite, "skill");
}

#[test]
#[ignore = "requires HIVE_NATIVE_CLAUDE; real CLI with scripted local HTTP, no account or model"]
fn full_access_native_skill_keeps_call_local_working_directory() {
    verify_native_skill(Sandbox::FullAccess, "skill-full-access");
}

fn verify_native_skill(sandbox: Sandbox, case: &'static str) {
    let mut f = NativeFixture::new(case);
    f.hive.start();
    let spawned = f.hive.ok(
        json!({"op":"spawn", "role":"native", "cwd":f.hive.tmp.path(),
               "sandbox":sandbox, "body":"NATIVE_ROOT_SKILL"}),
    );
    let id = spawned["agent_id"].as_str().unwrap();
    super::super::claude::answered(&f.hive, id, "NATIVE_SKILL_ROOT_DONE");
    f.assert_parent_output(id, "NATIVE_SKILL_ROOT_DONE");
    assert_call_local_directory(&f);
    assert_eq!(
        fs::read_to_string(f.hive.tmp.path().join("native-skill.txt")).unwrap(),
        "native-skill"
    );
    let rows = NativeFixture::rows(&f.trace);
    let tasks: Vec<_> = rows
        .iter()
        .filter(|row| row["subtype"] == "task_started")
        .collect();
    assert_eq!(tasks.len(), 1, "{rows:?}");
    assert!(
        tasks[0]["task_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(tasks[0]["tool_use_id"], "native-owner-skill");
    assert_eq!(tasks[0]["spawn_depth"], 1);
    assert_eq!(tasks[0]["is_backgrounded"], false);
    assert!(rows.iter().any(|row| row["kind"] == "decision"
        && row["agent_id"] == tasks[0]["task_id"]
        && row["tool"] == "mcp__hive__hive_agents"
        && row["decision"] == "deny"));
    assert_eq!(
        rows.iter()
            .filter(|row| row["kind"] == "sdk_dispatch")
            .count(),
        1
    );
    assert!(
        NativeFixture::rows(&f.receipts)
            .iter()
            .any(|row| row["parent_sdk_ok"] == true)
    );
    f.hive.ok(json!({"op":"terminate", "target":id}));
    f.hive.shutdown();
    f.assert_capture();
    f.export().unwrap();
}

fn assert_call_local_directory(f: &NativeFixture) {
    let cwd = f.hive.tmp.path();
    assert_eq!(
        fs::read_to_string(cwd.join("native cwd/same-call")).unwrap(),
        "nested"
    );
    assert_eq!(fs::read_to_string(cwd.join("next-call")).unwrap(), "root");
    assert!(!cwd.join("native cwd/next-call").exists());
    assert!(NativeFixture::rows(&f.receipts).iter().any(|row| {
        row["role"] == "NATIVE_ROOT_SKILL" && row["phase"] == 0 && row["bash_cwd_guidance"] == true
    }));
}

#[test]
#[ignore = "requires HIVE_NATIVE_CLAUDE; real CLI with scripted local HTTP, no account or model"]
fn interrupt_native_child_bash_then_continue_on_the_same_conversation() {
    let mut f = NativeFixture::new("interrupt");
    let release = support::ReleaseFile::new(f.hive.tmp.path().join("native-interrupt-release"));
    f.hive.start();
    let spawned = f.hive.ok(
        json!({"op":"spawn", "role":"native", "cwd":f.hive.tmp.path(),
        "body":"NATIVE_ROOT_INTERRUPT"}),
    );
    let id = spawned["agent_id"].as_str().unwrap();
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "native child guarded Bash",
        || {
            f.hive
                .tmp
                .path()
                .join("native-interrupt-ready")
                .exists()
                .then_some(())
        },
    );
    let child = identity::running(f.hive.tmp.path());
    let before = f.hive.status(id).thread_id.clone();
    interrupt_and_reap(&f.hive, id, &child);
    release.release();
    f.hive
        .ok(json!({"op":"send", "target":id, "body":"NATIVE_ROOT_AFTER"}));
    super::super::claude::answered(&f.hive, id, "NATIVE_AFTER_DONE");
    let resumed = f.hive.settled(id);
    assert_eq!(resumed.thread_id, before);
    assert!(resumed.error.is_none());
    let rows = NativeFixture::rows(&f.trace);
    let tasks: Vec<_> = rows
        .iter()
        .filter(|row| row["subtype"] == "task_started")
        .collect();
    assert_eq!(tasks.len(), 1, "{rows:?}");
    let task = tasks[0];
    assert_eq!(task["tool_use_id"], "native-owner-interrupt");
    assert!(task["task_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(task["is_backgrounded"], false);
    assert!(
        rows.iter().any(|row| row["kind"] == "decision"
            && row["hook"] == "PreToolUse"
            && row["tool_id"] == "native-interrupt-shell"
            && row["tool"] == "Bash"
            && row["agent_id"] == task["task_id"]
            && row["decision"] == "allow"),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row["kind"] == "owner_closed" && row["tool_id"] == "native-owner-interrupt"),
        "{rows:?}"
    );
    let requests = NativeFixture::rows(&f.receipts);
    assert_eq!(
        requests
            .iter()
            .filter(|row| row["role"] == "NATIVE_INTERRUPT_CHILD")
            .count(),
        1
    );
    f.hive.ok(json!({"op":"terminate", "target":id}));
    let stopped = f.hive.wait(id, |agent| agent.state == "stopped");
    assert!(stopped.error.is_none(), "{stopped:?}");
    f.hive.shutdown();
    f.assert_capture();
    f.export().unwrap();
}

fn interrupt_and_reap(f: &Harness, id: &str, child: &hive::state::ProcessIdentity) {
    f.ok(json!({"op":"interrupt", "target":id}));
    let interrupted = f.wait(id, |agent| {
        agent
            .last_finished_turn
            .as_ref()
            .is_some_and(|turn| turn.outcome == "interrupted")
    });
    assert!(interrupted.error.is_none(), "{interrupted:?}");
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "native interrupted Bash process disappearance",
        || child.absent().unwrap().then_some(()),
    );
}
