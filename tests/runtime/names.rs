use super::*;

fn spawn_named(f: &Harness, name: &str, body: &str) -> String {
    let reply = f.spawned(json!({"op":"spawn","role":"implementor","name":name,
        "cwd":f.tmp.path(),"body":body}));
    reply.agent_id.as_str().to_owned()
}

#[test]
fn names_label_agents_in_status_listings_trees_and_their_spawn_event() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let named = spawn_named(&f, "diff reviewer", "reply: hi");
    let plain = f.spawn("reply: hi");
    for id in [&named, &plain] {
        f.wait(id, |a| a.state == "idle");
    }
    assert_eq!(f.status(&named).name.as_deref(), Some("diff reviewer"));
    // An unnamed agent has no name key, so its saved record is unchanged.
    assert!(f.status(&plain).name.is_none());
    let agents = f.listed(json!({"op":"agents","limit":100}));
    assert_eq!(
        agents
            .agents
            .iter()
            .find(|a| a.id == named)
            .unwrap_or_else(|| panic!("{named} not in {:?}", agents.agents))
            .name
            .as_deref()
            .unwrap(),
        "diff reviewer"
    );
    let tree = f.tree(json!({"op":"tree","all":true}));
    assert_eq!(
        tree.agents
            .iter()
            .find(|a| a.id == named)
            .unwrap_or_else(|| panic!("{named} not in {:?}", tree.agents))
            .name
            .as_deref()
            .unwrap(),
        "diff reviewer"
    );
    assert!(
        tree.agents
            .iter()
            .find(|a| a.id == plain)
            .unwrap_or_else(|| panic!("{plain} not in {:?}", tree.agents))
            .name
            .is_none()
    );
    let spawned = f
        .events(&named)
        .events
        .into_iter()
        .find(|event| event.kind == "spawned")
        .unwrap();
    assert_eq!(spawned.data["name"], "diff reviewer");
    // A question names its agent.
    let asking = spawn_named(
        &f,
        "asker",
        r#"ask:{"message":"Go?","request_id":"question-1"}"#,
    );
    f.wait(&asking, |a| {
        a.state == "idle" && a.waiting_on.as_deref() == Some("human")
    });
    let approvals = f.ok(json!({"op":"approvals"}));
    let question = approvals["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["agent_id"] == asking.as_str())
        .unwrap_or_else(|| panic!("{approvals}"));
    assert_eq!(question["agent_name"], "asker");
    f.shutdown();
}

#[test]
fn set_name_changes_or_clears_at_once_and_refuses_bad_names() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = spawn_named(&f, "first", "reply: hi");
    f.settled(&id);
    assert_eq!(
        f.ok(json!({"op":"set_name","target":id,"name":"lead"})),
        json!({"agent_id":id,"name":"lead"})
    );
    assert_eq!(f.status(&id).name.as_deref(), Some("lead"));
    assert_eq!(
        f.ok(json!({"op":"set_name","target":id,"clear":true})),
        json!({"agent_id":id,"name":null})
    );
    assert!(f.status(&id).name.is_none());
    let refused = |args: Value, code: &str, field: &str| {
        let reply = f.expect_err(args, code);
        assert_eq!(reply.error.field.as_deref().unwrap(), field, "{reply:?}");
    };
    refused(
        json!({"op":"set_name","target":id}),
        "missing_argument",
        "/name",
    );
    refused(
        json!({"op":"set_name","target":id,"name":"x","clear":true}),
        "conflicting_arguments",
        "/clear",
    );
    for bad in [
        String::new(),
        " lead".into(),
        "a".repeat(65),
        "two\nlines".into(),
    ] {
        refused(
            json!({"op":"set_name","target":id,"name":bad}),
            "invalid_value",
            "/name",
        );
        refused(
            json!({"op":"spawn","role":"implementor","name":bad,"cwd":f.tmp.path(),"body":"x"}),
            "invalid_value",
            "/name",
        );
    }
    f.expect_err(
        json!({"op":"set_name","target":"00000000-0000-4000-8000-000000000009","name":"x"}),
        "unknown_agent",
    );
    // A retry returns the original reply and does not rename again.
    let args = json!({"op":"set_name","target":id,"name":"retried"});
    let first = f.call_id("rename-1", args.clone());
    f.ok(json!({"op":"set_name","target":id,"name":"later"}));
    assert_eq!(f.call_id("rename-1", args), first);
    assert_eq!(f.status(&id).name.as_deref(), Some("later"));
    // An ended agent can still be renamed.
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| a.ended_at.is_some());
    f.ok(json!({"op":"set_name","target":id,"name":"archived"}));
    assert_eq!(f.status(&id).name.as_deref(), Some("archived"));
    f.shutdown();
}

#[test]
fn hosted_agents_and_the_cli_rename_agents() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let target = f.spawn("reply: hi");
    f.settled(&target);
    let call = json!({"name":"hive_set_name","arguments":{
        "target":target,"name":"named by a peer","request_id":"peer-name-1"}});
    let caller = f.spawn(&format!("tool:{call}"));
    let reply = f.settled(&caller);
    assert!(reply.error.is_none(), "{reply:?}");
    assert_eq!(f.status(&target).name.as_deref(), Some("named by a peer"));
    let cli = |args: &[&str]| {
        support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(args)
            .output()
            .unwrap()
    };
    let output = cli(&["set-name", &target, "from the cli"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(f.status(&target).name.as_deref(), Some("from the cli"));
    let output = cli(&["set-name", &target, "--clear"]);
    assert!(output.status.success(), "{output:?}");
    assert!(f.status(&target).name.is_none());
    for refused in [
        &["set-name", target.as_str()][..],
        &["set-name", target.as_str(), "x", "--clear"],
        &["set-name", target.as_str(), " padded"],
    ] {
        assert_eq!(cli(refused).status.code(), Some(2), "{refused:?}");
    }
    f.shutdown();
}

#[test]
fn batch_and_branch_spawns_take_names_and_the_cli_maps_a_refused_one() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let batch = f.batch::<harness::views::SpawnView>(json!({"op":"batch","operations":[
        {"op":"spawn","role":"implementor","name":"from a batch","cwd":f.tmp.path(),
         "body":"reply: hi","request_id":"named-1"}]}));
    let child = batch
        .results
        .first()
        .unwrap_or_else(|| panic!("{batch:?}"))
        .agent_id
        .as_str();
    let child = child.to_owned();
    f.settled(&child);
    assert_eq!(f.status(&child).name.as_deref(), Some("from a batch"));
    // A branch is named on its own, never after its source.
    let branch = f.ok(json!({"op":"spawn","from_agent":child,"name":"the branch",
        "body":"reply: branched"}));
    let branch = branch["agent_id"].as_str().unwrap().to_owned();
    f.settled(&branch);
    assert_eq!(f.status(&branch).name.as_deref(), Some("the branch"));
    let unnamed = f.ok(json!({"op":"spawn","from_agent":child,"body":"reply: again"}));
    let unnamed = unnamed["agent_id"].as_str().unwrap().to_owned();
    f.settled(&unnamed);
    assert!(f.status(&unnamed).name.is_none());
    // The CLI names the operations file for a name the daemon refuses.
    let operations = f.tmp.path().join("operations.json");
    support::publish_fixture(
        &operations,
        &serde_json::to_vec(&json!([{"op":"spawn","role":"implementor","name":" padded",
            "cwd":f.tmp.path(),"body":"x","request_id":"bad-name"}]))
        .unwrap(),
    );
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(["batch", "--operations-file"])
        .arg(&operations)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let refused: Value = serde_json::from_slice(&output.stdout).unwrap();
    let refused = support::reply::Failure::parse(refused);
    assert_eq!(refused.error.code, "invalid_value", "{refused:?}");
    assert_eq!(
        refused.error.field.as_deref().unwrap(),
        "--operations-file",
        "{refused:?}"
    );
    f.shutdown();
}
