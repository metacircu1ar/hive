use super::*;
use crate::{
    config::{Method, Profile},
    runtime::{
        Core,
        api::RequestEnvelope,
        auth_fallback::{AuthReason, AvailabilityState, prepare},
        caller::Caller,
        preflight::request::{Facts, RequestInput},
        provider::protocol::Model,
        test_support::{AgentBuilder, CoreBuilder},
    },
};
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path};

fn model(name: &str) -> Vec<Model> {
    vec![Model::parse(&json!({"model":name,
        "supportedReasoningEfforts":[{"reasoningEffort":"low"}]}))]
}

fn setup(directory: &Path) -> (Core, prepare::Prepared) {
    let mut core = CoreBuilder::new(directory).build();
    let key = directory.canonicalize().unwrap().join("synthetic-key");
    std::fs::write(&key, "test-only-key").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let subscription = AgentBuilder::new("unused").build().profile;
    let api = Profile {
        method: Method::ApiKey,
        api_key_env: None,
        api_key_file: Some(key),
        ..subscription.clone()
    };
    for (name, profile) in [("main", subscription), ("api", api)] {
        core.state().register(name, &profile).unwrap();
        core.config_mut().auth_profiles.insert(name.into(), profile);
    }
    let mut group = AuthGroup::ordered(vec!["main".into()]);
    group.paid_tail.push("api".into());
    core.config_mut().auth_groups.insert("plans".into(), group);
    let mut prepared = prepare::Input::capture(&core, "plans".into())
        .resolve()
        .unwrap();
    for member in &mut prepared.members {
        member.catalog = Some(Ok(model(if member.auth == "api" {
            "only-api"
        } else {
            "other"
        })));
        core.store()
            .select_key_authority(
                &member.auth,
                member.binding.as_ref().unwrap().id,
                member.authority(),
            )
            .unwrap();
    }
    (core, prepared)
}

fn request<'a>(core: &'a Core, prepared: &'a prepare::Prepared) -> SelectionRequest<'a> {
    SelectionRequest {
        queries: core.store(),
        members: &prepared.members,
        model: "only-api",
        effort: "low",
        follow_availability: true,
        allow_api: true,
        sticky: None,
        agent: None,
        group: core.config().auth_groups.get("plans"),
        capacity: None,
    }
}

fn hold(core: &Core, member: &MemberFacts, seconds: u64) {
    let mut value = core
        .store()
        .availability(&member.auth, member.binding.as_ref().unwrap().id)
        .unwrap();
    value.state = AvailabilityState::Limited;
    value.reason = Some(AuthReason::UsageLimit);
    value.until = Some(crate::state::now() + seconds);
    core.store().save_revised(&mut value).unwrap();
}

#[test]
fn an_incompatible_unexhausted_plan_cannot_park_on_paid_api_or_misreport_a_catalog_miss() {
    let directory = crate::runtime::test_directory();
    let (mut core, prepared) = setup(directory.path());
    hold(&core, &prepared.members[1], 60);
    let error = select(request(&core, &prepared)).err().unwrap();
    assert_eq!(error.code(), "auth_fallback_blocked");
    assert!(std::error::Error::source(&error).is_none());
    let envelope = RequestEnvelope::new(json!({"op":"spawn", "auth":"plans",
        "model":"only-api", "effort":"low", "cwd":directory.path(), "body":"work"}));
    let mut facts: Facts = RequestInput::capture(&core, &Caller::Human, &envelope).resolve();
    for spawn in facts.spawns.values_mut() {
        for member in &mut spawn.members {
            member.catalog = Some(Ok(model(if member.auth == "api" {
                "only-api"
            } else {
                "other"
            })));
        }
    }
    assert_eq!(
        core.dispatch(&Caller::Human, "blocked-spawn", &envelope, &facts)
            .err()
            .unwrap()
            .code(),
        "auth_fallback_blocked"
    );
    assert_eq!(core.store().active_count().unwrap(), 0);
    assert!(!core.store().request_used("human", "blocked-spawn").unwrap());
}

#[test]
fn waiting_api_requires_all_plan_exhaustion_but_manual_and_sticky_choices_keep_their_rules() {
    let directory = crate::runtime::test_directory();
    let (core, prepared) = setup(directory.path());
    hold(&core, &prepared.members[1], 60);
    let mut explicit = request(&core, &prepared);
    explicit.follow_availability = false;
    assert!(!select(explicit).unwrap().waiting());
    let mut sticky = request(&core, &prepared);
    sticky.sticky = Some("api");
    assert!(select(sticky).unwrap().waiting());
    hold(&core, &prepared.members[0], 3600);
    let waiting = select(request(&core, &prepared)).unwrap();
    assert_eq!(waiting.index, 1);
    assert!(waiting.waiting());
    let member = &prepared.members[0];
    let mut failed = core
        .store()
        .availability("main", member.binding.as_ref().unwrap().id)
        .unwrap();
    failed.state = AvailabilityState::Unavailable;
    failed.reason = Some(AuthReason::Authentication);
    core.store().save_revised(&mut failed).unwrap();
    assert_eq!(
        select(request(&core, &prepared)).err().unwrap().code(),
        "auth_fallback_blocked"
    );
}

#[test]
fn ineligible_paid_reset_does_not_shorten_the_subscription_waiting_deadline() {
    let directory = crate::runtime::test_directory();
    let (core, mut prepared) = setup(directory.path());
    prepared.members[0].catalog = Some(Ok(model("only-api")));
    hold(&core, &prepared.members[0], 3600);
    hold(&core, &prepared.members[1], 60);
    let expected = core
        .store()
        .availability("main", prepared.members[0].binding.as_ref().unwrap().id)
        .unwrap()
        .until;
    let mut request = request(&core, &prepared);
    request.allow_api = false;
    let waiting = select(request).unwrap();
    assert_eq!(waiting.index, 0);
    assert_eq!(waiting.deadline(), expected);
}

#[test]
fn explicit_api_opt_out_has_an_accurate_error_even_after_all_plans_are_exhausted() {
    let directory = crate::runtime::test_directory();
    let (core, prepared) = setup(directory.path());
    hold(&core, &prepared.members[0], 3600);
    let mut request = request(&core, &prepared);
    request.allow_api = false;
    let error = select(request).err().unwrap();
    assert_eq!(error.code(), "auth_fallback_blocked");
    assert_eq!(
        error.message(),
        "API fallback is disabled for this agent or role; \
        no subscription supports this model and effort."
    );
    assert!(std::error::Error::source(&error).is_none());
}
