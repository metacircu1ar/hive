//! Spawn validation regressions.
use super::*;

#[test]
fn spawn_validation_order_and_late_body_failure_preserve_rollback() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    assert_spawn_capacity_precedes_role_validation(&mut core);
    control_inputs::configure_spawn(&mut core);
    assert_error(
        call(
            &mut core,
            "human",
            "cwd",
            json!({"op":"spawn","role":"test","cwd":"relative"}),
        ),
        InputContext::Spawn.error(
            hive_input::Code::InvalidPath,
            "/cwd",
            "an absolute UTF-8 directory path without NUL bytes",
        ),
    );
    assert_error(
        call(
            &mut core,
            "human",
            "instructions",
            json!({"op":"spawn","role":"test","cwd":temp.path()}),
        ),
        Error::instructions_unavailable("Cannot read a configured instruction file."),
    );
    std::fs::write(temp.path().join("role.md"), "instructions").unwrap();
    let no_body = json!({"op":"spawn","role":"test","cwd":temp.path()});
    control_inputs::assert_input(
        call(&mut core, "human", "body", no_body.clone()),
        "missing_argument",
        "/body",
        "spawn",
    );
    assert_eq!(core.store().active_count().unwrap(), 0);
    assert!(
        core.store()
            .prior("human", "body", &no_body.to_string())
            .unwrap()
            .is_none()
    );
    control_inputs::malformed_spawn_fields(&mut core, temp.path());
    let accepted = call(
        &mut core,
        "human",
        "body",
        json!({
            "op": "spawn",
            "role": "test",
            "cwd": temp.path(),
            "body": "work",
            "model": "mock"
        }),
    )
    .unwrap();
    let saved = core
        .store()
        .agent(accepted["agent_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved.model, "mock");
    assert_eq!(saved.auth, "main");
    assert_eq!(saved.effort, "high");
}

fn assert_spawn_capacity_precedes_role_validation(core: &mut Core) {
    core.config_mut().runtime.max_agents = 0;
    assert_error(
        call(core, "human", "capacity", json!({"op":"spawn"})),
        Error::agent_limit("Configured nonterminal or draining agent capacity is exhausted."),
    );
    core.config_mut().runtime.max_agents = 128;
    assert_error(
        call(core, "human", "role", json!({"op":"spawn","role":"absent"})),
        InputContext::Spawn.error(
            hive_input::Code::InvalidValue,
            "/role",
            "a configured role name",
        ),
    );
}
