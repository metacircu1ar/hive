//! Outside CLI observation never cancels the accepted one-shot task.
use super::*;
use std::path::Path;

pub(super) fn cli(f: &Harness, args: &[&str]) -> (Value, std::process::Output) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
    (value, output)
}

provider_matrix!(
    one_shot_cli_wait_timeout_then_observe_completion,
    timeout_case
);

fn timeout_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let role = if provider == Provider::Claude {
        "claude"
    } else {
        "implementor"
    };
    let (timeout, output) = cli(
        &f,
        &[
            "spawn",
            role,
            "--cwd",
            f.tmp.path().to_str().unwrap(),
            "--body",
            "hold",
            "--one-shot",
            "--wait",
            "--timeout",
            "1",
            "--request-id",
            "wait-once",
        ],
    );
    assert!(!output.status.success(), "{timeout}");
    assert_eq!(timeout["wait"], "timeout");
    let id = timeout["agent_id"].as_str().unwrap();
    assert_eq!(timeout["agent"]["id"], id);
    assert!(String::from_utf8(output.stderr).unwrap().contains(id));
    let running = f.wait(id, |a| a.state == "running");
    assert_eq!(running.task_outcome, "open");
    assert!(!running.paused);
    assert_eq!(
        f.ok(json!({"op":"tree","all":true}))["agents"][0]["one_shot"],
        true
    );
    f.ok(json!({"op":"send","target":id,"body":"reply:done","mode":"redirect"}));
    let (result, output) = cli(&f, &["wait", id, "--until", "completed", "--timeout", "10"]);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["agent_id"], id);
    assert_eq!(result.as_object().unwrap().len(), 4);
    let completed = f.status(id);
    assert_eq!(
        result["result"],
        serde_json::to_value(&completed.result).unwrap()
    );
    assert_eq!(
        result["result_selection"],
        serde_json::to_value(&completed.result_selection).unwrap()
    );
    assert_eq!(completed.thread_id, running.thread_id);
    assert_eq!(completed.one_shot_waiting, "none");
    f.shutdown();
}

#[test]
fn one_shot_cli_sigint_leaves_accepted_task_running() {
    use std::os::unix::process::ExitStatusExt;
    let mut f = Harness::new();
    f.start();
    let receipt = f.tmp.path().join("wait.stderr");
    let mut observer = start_task_observer(&f, &receipt);
    let id = accepted_task(&f, &receipt);
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&observer),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let exit = support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "observer exit",
        || observer.try_wait().unwrap(),
    );
    assert_eq!(exit.signal(), Some(libc::SIGINT));
    assert_eq!(f.status(&id).state, "running");
    f.ok(json!({"op":"send","target":id,"body":"reply:after wait","mode":"redirect"}));
    f.wait(&id, |a| a.task_outcome == "completed");
    f.shutdown();
}

fn start_task_observer(f: &Harness, receipt: &Path) -> support::ChildGuard {
    support::ChildGuard::new(
        support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(["spawn", "implementor", "--cwd"])
            .arg(f.tmp.path())
            .args([
                "--body",
                "hold",
                "--name",
                "interrupt-wait",
                "--one-shot",
                "--wait",
            ])
            .stdout(Stdio::null())
            .stderr(fs::File::create(receipt).unwrap())
            .spawn()
            .unwrap(),
    )
}

fn accepted_task(f: &Harness, receipt: &Path) -> String {
    let id = support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "accepted task",
        || {
            f.ok(json!({"op":"agents"}))["agents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["name"] == "interrupt-wait")
                .map(|a| a["id"].as_str().unwrap().to_owned())
        },
    );
    f.wait(&id, |a| a.state == "running");
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "acceptance receipt",
        || {
            fs::read_to_string(receipt)
                .unwrap()
                .contains(&id)
                .then_some(())
        },
    );
    id
}
