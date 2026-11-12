//! One-shot admission and strict draft contracts, without a provider process.
use super::*;

fn configured(path: &Path) -> Core {
    let mut c = core(path);
    control_inputs::configure_spawn(&mut c);
    c
}

fn spawn(path: &Path) -> Value {
    json!({"op":"spawn","role":"test","cwd":path,"body":"review",
        "instructions":"","sandbox":"read-only"})
}

#[test]
fn one_shot_is_explicit_for_human_and_hosted_spawns_and_visible_in_records() {
    for caller in ["human", "parent"] {
        let temp = crate::runtime::test_directory();
        let mut c = configured(temp.path());
        let mut parent = agent("parent");
        parent.cwd = temp.path().to_owned();
        parent.one_shot = true;
        c.store().save(&parent).unwrap();
        for (key, selection) in [
            ("default", None),
            ("false", Some(false)),
            ("true", Some(true)),
        ] {
            let mut request = spawn(temp.path());
            if let Some(value) = selection {
                request["one_shot"] = json!(value);
            }
            let reply = call(&mut c, caller, key, request.clone()).unwrap();
            assert_eq!(call(&mut c, caller, key, request).unwrap(), reply);
            let id = reply["agent_id"].as_str().unwrap();
            let saved = c.store().agent(id).unwrap();
            assert_eq!(saved.one_shot, selection == Some(true));
            assert_eq!(
                saved.parent_id.as_deref(),
                (caller != "human").then_some(caller)
            );
            let status = call(
                &mut c,
                "human",
                "status",
                json!({"op":"status","target":id}),
            )
            .unwrap();
            let decoded = DaemonReply::new(&status).unwrap();
            assert_eq!(decoded.agent().unwrap().raw()["one_shot"], saved.one_shot);
            for key in [
                "completed_by",
                "result_selection",
                "result_read_target",
                "completion_pending_by",
            ] {
                assert_eq!(status["agent"][key], Value::Null);
            }
            let events = crate::api::value(c.store().events(id, 0, 20).unwrap());
            assert_eq!(events["events"][0]["data"]["one_shot"], saved.one_shot);
        }
    }
}

#[test]
fn one_shot_rejects_non_booleans_and_is_not_a_role_setting() {
    assert!(serde_json::from_value::<Role>(json!({"one_shot":true})).is_err());
    let temp = crate::runtime::test_directory();
    let mut c = configured(temp.path());
    for invalid in [json!(null), json!("true"), json!(1), json!([]), json!({})] {
        let mut request = spawn(temp.path());
        request["one_shot"] = invalid;
        control_inputs::assert_input(
            call(&mut c, "human", "bad", request),
            "invalid_value",
            "/one_shot",
            "spawn",
        );
        assert_eq!(c.store().active_count().unwrap(), 0);
    }
    for value in [false, true] {
        validate_tool(
            "hive_spawn",
            &json!({"body":"work","one_shot":value,"request_id":"child"}),
        )
        .unwrap();
    }
    assert!(
        validate_tool(
            "hive_spawn",
            &json!({"body":"work","one_shot":null,"request_id":"child"})
        )
        .is_err()
    );
}

#[test]
fn one_shot_join_scope_belongs_to_spawns_not_sends() {
    let temp = crate::runtime::test_directory();
    let mut c = configured(temp.path());
    let mut target = agent("existing");
    target.one_shot = true;
    c.store().save(&target).unwrap();
    let mut first = spawn(temp.path());
    first["one_shot"] = json!(true);
    first["request_id"] = json!("one");
    let mut ordinary = spawn(temp.path());
    ordinary["request_id"] = json!("ordinary");
    let reply = call(&mut c, "human", "round", json!({"op":"batch","join":{},"operations":[
        first, ordinary, {"op":"send","target":"existing","body":"follow-up","request_id":"send"}
    ]})).unwrap();
    let connection = rusqlite::Connection::open(temp.path().join("runtime.sqlite")).unwrap();
    let mut statement = connection
        .prepare("SELECT task_scoped FROM join_members WHERE join_id=?1 ORDER BY position")
        .unwrap();
    let scopes = statement
        .query_map([reply["join"]["join_id"].as_str().unwrap()], |row| {
            row.get::<_, bool>(0)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(scopes, [true, false, false]);
    assert!(
        connection
            .execute("UPDATE join_members SET task_scoped=2", [])
            .is_err()
    );
    assert!(
        connection
            .execute("UPDATE join_members SET task_scoped=NULL", [])
            .is_err()
    );
}
