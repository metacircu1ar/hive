//! The same validation, pagination and decoding for transactional and control reads.
mod filters;
use super::{
    api::{self, Request},
    invalid_request,
    store::Queries,
};
use crate::error::Result;
use crate::runtime::api::InputContext as Op;
use api::pagination::{bytes, number, rows};
use serde_json::Value;

#[derive(Clone)]
pub(super) struct VerificationContext {
    config: String,
    root: String,
    binary: String,
}
impl VerificationContext {
    pub(super) fn new(
        config: &std::path::Path,
        root: &std::path::Path,
        binary: &std::path::Path,
    ) -> Result<Self> {
        fn absolute(path: &std::path::Path) -> Result<String> {
            std::path::absolute(path)
                .map_err(crate::error::Error::state_io_from)?
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    crate::error::Error::invalid_path("Verification argv requires UTF-8 paths.")
                })
        }
        Ok(Self {
            config: absolute(config)?,
            root: absolute(root)?,
            binary: absolute(binary)?,
        })
    }
    fn argv(&self, auth: &str, model: &str, effort: &str) -> Vec<String> {
        vec![
            "hive".into(),
            "--config".into(),
            self.config.clone(),
            "--state-dir".into(),
            self.root.clone(),
            "--claude-bin".into(),
            self.binary.clone(),
            "verify-steering".into(),
            "--auth".into(),
            auth.into(),
            "--model".into(),
            model.into(),
            "--effort".into(),
            effort.into(),
        ]
    }
}

// Agent lists use the largest public page; history reads use its smaller default.
const AGENT_PAGE_ROWS: u64 = crate::limits::MAX_PAGE_ROWS;
const HISTORY_PAGE_ROWS: u64 = crate::limits::DEFAULT_HISTORY_PAGE_ROWS;
// A tree reply carries at most this many agents and reports truncation.
pub(super) const TREE_ROWS: usize = 1000;
// Output pages share the control-plane retained-text cap.
const OUTPUT_BYTES: u64 = crate::limits::CONTROL_INPUT_BYTES as u64;

pub(super) fn dispatch(
    queries: &Queries,
    request: &Request,
    verification: Option<&VerificationContext>,
    config: &crate::config::Config,
) -> Result<api::Response> {
    let page_error = |error: api::pagination::InvalidPage| error.for_request(request);
    match request {
        #[cfg(feature = "comb")]
        Request::Collectors => queries.collector_status().map(|destinations| {
            api::Response::from(api::CollectorReply {
                collectors: destinations,
            })
        }),
        Request::OptionsCatalog(args) => crate::options::listing(
            args.provider
                .optional(Op::OptionsCatalog, "/provider", "codex or claude")?
                .copied(),
            args.model
                .optional(Op::OptionsCatalog, "/model", "a model token")?
                .map(String::as_str),
        )
        .map(api::Response::from),
        Request::AuthGroups => queries.auth_groups(config).map(api::Response::from),
        Request::Roles => Ok(api::Response::from(api::RolesResponse::new(config))),
        Request::AuthAvailability(args) => queries
            .auth_status(args.auth.required(Op::AuthAvailability, "/auth")?)
            .map(api::Response::from),
        Request::Agents(args) if args.stalled_requested() => {
            rows(number(&args.limit, HISTORY_PAGE_ROWS, "/limit").map_err(page_error)?)
                .map_err(page_error)?;
            queries
                .stalled_groups(args, crate::state::now())
                .map(api::Response::from)
        }
        Request::Agents(args) => filters::agents(queries, args, config).map(api::Response::from),
        Request::Status(args) => status(queries, args, verification).map(api::Response::from),
        Request::Tree(args) => tree(queries, args).map(api::Response::from),
        Request::Attach(args) => attach(queries, args, verification).map(api::Response::from),
        Request::Read(args) => history(queries, args, request).map(api::Response::from),
        Request::Messages(args) => queries
            .messages(
                args.target.required(Op::Messages, "/target")?,
                match &args.before {
                    api::Input::Missing | api::Input::Invalid(Value::Null) => None,
                    value => Some(number(value, 0, "/before").map_err(page_error)?),
                },
                rows(number(&args.limit, HISTORY_PAGE_ROWS, "/limit").map_err(page_error)?)
                    .map_err(page_error)?,
            )
            .map(api::Response::from),
        Request::MessageResult(args) => queries
            .message_result(args.message_id.required(Op::MessageResult, "/message_id")?)
            .map(api::Response::from),
        Request::Approvals(args) => queries.approvals(args).map(api::Response::from),
        Request::Approval(args) => queries
            .approval(api::ApprovalInput::Show.id(&args.approval_id)?)
            .map(|approval| api::Response::from(crate::approvals::ApprovalResponse { approval })),
        Request::Output(args) => queries
            .output(
                args.id.required(Op::Output, "/id")?,
                number(&args.offset, 0, "/offset").map_err(page_error)?,
                usize::try_from(
                    bytes(number(&args.limit, OUTPUT_BYTES, "/limit").map_err(page_error)?)
                        .map_err(page_error)?,
                )
                .map_err(|cause| invalid_request().with_source(cause))?,
            )
            .map(api::Response::from),
        _ => Err(invalid_request()),
    }
}

fn status(
    queries: &Queries,
    args: &api::StatusArgs,
    verification: Option<&VerificationContext>,
) -> Result<api::AgentResponse<ApprovalStatus>> {
    let mut agent = queries.agent(args.target.required(Op::Status, "/target")?)?;
    if args
        .instructions
        .optional(Op::Status, "/instructions", "a boolean")?
        .copied()
        == Some(true)
    {
        agent.instructions = queries.instructions(&agent.id)?;
    }
    let steering_verification = verification_targets(&agent, verification);
    let approval_id = queries.pending_approval(&agent.id)?;
    let mut branch = queries.branch_intent(&agent.id)?;
    if let Some(branch) = &mut branch {
        branch.source_handoff = None;
    }
    let branch_dependents = queries.branch_dependents(&agent.id)?;
    let open_joins = queries.open_joins(&agent.id)?;
    let agent = ApprovalStatus {
        automatic_continuation: queries.automatic_continuation(&agent)?,
        open_joins: open_joins.ids,
        open_joins_truncated: open_joins.truncated,
        listing: api::ListingFields {
            one_shot_waiting: queries.one_shot_waiting(&agent)?,
            held_for_build: queries.agent_hold(&agent.id)?,
            idle_since: queries.idle_since(&agent.id)?,
            provider: agent.profile.provider,
            billing: agent.profile.method,
            waiting_on: approval_id.as_ref().map(|_| "human"),
            approval_id,
        },
        agent: agent.public(),
        branch,
        branch_dependents,
    };
    Ok(api::AgentResponse {
        attachment_store_bytes: queries.attachment_bytes()?,
        latest_attachments: queries
            .latest_attachments(args.target.required(Op::Status, "/target")?)?,
        agent,
        steering_verification,
    })
}

/// Compact rows of one subtree, or of the live roots, from one snapshot.
fn tree(queries: &Queries, args: &api::TreeArgs) -> Result<api::TreeResponse> {
    let (root, all) = tree_selection(args)?;
    let ids = if all {
        queries.live_ids(TREE_ROWS)?
    } else {
        queries.tree_ids(root, TREE_ROWS)?
    };
    let truncated = ids.len() > TREE_ROWS;
    let agents = ids
        .iter()
        .take(TREE_ROWS)
        .map(|id| queries.tree_row(&queries.agent(id)?))
        .collect::<Result<_>>()?;
    Ok(api::TreeResponse { agents, truncated })
}

/// Pure selectors are admitted before a control long-poll consumes a permit.
pub(super) fn tree_selection(args: &api::TreeArgs) -> Result<(Option<&str>, bool)> {
    let root = args
        .root
        .optional(Op::Tree, "/root", "an agent ID")?
        .map(String::as_str);
    let all = args
        .all
        .optional(Op::Tree, "/all", "a boolean")?
        .copied()
        .unwrap_or(false);
    if all && root.is_some() {
        return Err(Op::Tree.error(
            hive_input::Code::ConflictingArguments,
            "/all",
            "either root or all, not both",
        ));
    }
    Ok((root, all))
}

/// The agent's status, newest event page and counts, from one snapshot.
fn attach(
    queries: &Queries,
    args: &api::AttachArgs,
    verification: Option<&VerificationContext>,
) -> Result<api::AttachResponse> {
    let target = args.target.required(Op::Attach, "/target")?;
    let invalid = |error: api::pagination::InvalidPage| {
        Op::Attach.error(error.code, error.field, error.expected)
    };
    let limit = rows(number(&args.limit, HISTORY_PAGE_ROWS, "/limit").map_err(invalid)?)
        .map_err(invalid)?;
    let cursor = |value: &api::Input<u64>, field| match value {
        api::Input::Missing => Ok(None),
        value => number(value, 0, field).map(Some).map_err(invalid),
    };
    let (after, before) = (
        cursor(&args.after, "/after")?,
        cursor(&args.before, "/before")?,
    );
    if after.is_some() && before.is_some() {
        return Err(Op::Attach.error(
            hive_input::Code::ConflictingArguments,
            "/before",
            "either after or before, not both",
        ));
    }
    if before == Some(0) {
        return Err(Op::Attach.error(
            hive_input::Code::OutOfRange,
            "/before",
            "an event sequence of at least 1",
        ));
    }
    let status = status(
        queries,
        &api::StatusArgs {
            target: args.target.clone(),
            ..api::StatusArgs::default()
        },
        verification,
    )?;
    let events = match (after, before) {
        (_, Some(before)) => {
            api::ReadOutput::History(queries.events_before(target, before, limit)?)
        }
        (Some(after), None) => queries.events(target, after, limit)?,
        (None, None) => queries.events(target, queries.tail_after(target, limit)?, limit)?,
    };
    let counts = queries.counts(target)?;
    Ok(api::AttachResponse {
        agent: status.agent,
        events,
        queued: counts.queued,
        mail: counts.mail,
        children: counts.children,
    })
}

// Artifacts are immutable until explicit purge; its reader barrier precedes unlink.
// Their manifest SELECT ends before file I/O; no WAL snapshot spans a file read.
pub(super) fn artifact(request: &Request) -> bool {
    matches!(request, Request::Output(_))
        || matches!(request, Request::Read(args) if args.target.as_str()
            .is_some_and(|target| super::domain::ReadTarget::parse(target).artifact().is_some()))
}

pub(super) struct RuntimeDiagnostics {
    pub storage: api::StorageDiagnostics,
    pub resident_actors: usize,
    pub profile_leases: usize,
    pub writer_observation: api::WriterObservation,
}
pub(super) fn ping(
    binary_sha256: String,
    provider_selection: super::ProviderSelection,
    diagnostics: RuntimeDiagnostics,
) -> api::PingResponse {
    let RuntimeDiagnostics {
        storage,
        resident_actors,
        profile_leases,
        writer_observation,
    } = diagnostics;
    api::PingResponse {
        binary_sha256,
        commit: hive_setup_support::identity::COMMIT,
        protocols: crate::compatibility::CONTROL_RANGE,
        agent_catalog: crate::compatibility::AGENT_CATALOG.current,
        state_format: crate::compatibility::STATE.current,
        provider_compatibility: crate::provider_support::REGISTRIES,
        provider_selection,
        configuration: None,
        deprecations: Vec::new(),
        managed_service: hive_setup_support::managed::label(),
        product_version: env!("CARGO_PKG_VERSION"),
        compiled_features: super::features::COMPILED,
        comb_backlog: None,
        status: api::ResponseStatus::Running,
        pid: std::process::id(),
        storage,
        resident_actors,
        profile_leases,
        writer_observation,
        capabilities: api::Capabilities {
            providers: [
                api::ProviderCapabilities {
                    provider: "codex",
                    subscription: true,
                    next_turn: true,
                    redirect: true,
                    steering_policy: api::SteeringPolicy::NativeProbe,
                    read_only_shell: true,
                },
                api::ProviderCapabilities {
                    provider: "claude",
                    subscription: true,
                    next_turn: true,
                    redirect: true,
                    steering_policy: api::SteeringPolicy::VerifiedCanary,
                    read_only_shell: false,
                },
            ],
            approvals: concat!(
                "advisory asynchronous human questions; ",
                "native provider approvals remain disabled"
            ),
        },
    }
}

#[derive(serde::Serialize)]
pub(crate) struct ApprovalStatus {
    pub automatic_continuation: super::store::AutomaticContinuation,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    open_joins: Vec<String>,
    open_joins_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<super::branching::Intent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    branch_dependents: Vec<String>,
    #[serde(flatten)]
    listing: api::ListingFields,
    #[serde(flatten)]
    agent: super::domain::PublicAgent,
}

fn verification_targets(
    agent: &super::domain::Agent,
    context: Option<&VerificationContext>,
) -> Vec<api::SteeringVerification> {
    use super::domain::AgentState;
    if agent.profile.provider != crate::config::Provider::Claude || agent.steering.supported() {
        return vec![];
    }
    let target = |selection: super::domain::OptionsSelection, source| api::SteeringVerification {
        argv: context.map(|context| context.argv(&agent.auth, &selection.model, &selection.effort)),
        auth: agent.auth.clone(),
        model: selection.model,
        effort: selection.effort,
        selection: source,
        daemon_must_be_stopped: true,
        enables_next_launch: true,
    };
    let next = agent.boundary_selection();
    let current = matches!(agent.state, AgentState::Idle | AgentState::Running)
        .then_some(agent.session_selection.as_ref())
        .flatten();
    let same =
        current.is_some_and(|session| session.model == next.model && session.effort == next.effort);
    current
        .map(|session| target(session.clone(), api::VerificationSelection::CurrentSession))
        .into_iter()
        .chain((!same).then(|| target(next, api::VerificationSelection::NextTurn)))
        .collect()
}

fn history(queries: &Queries, args: &api::ReadArgs, request: &Request) -> Result<api::ReadOutput> {
    let page_error = |error: api::pagination::InvalidPage| error.for_request(request);
    args.target.required(Op::Read, "/target")?;
    let page = args.page().map_err(page_error)?;
    if let super::domain::ReadTarget::Join { id: join, .. } = page.target {
        queries
            .find_join_snapshot(join)?
            .map(api::ReadOutput::Join)
            .ok_or_else(|| {
                Op::ReadJoin.error(
                    hive_input::Code::InvalidValue,
                    "/target",
                    "a currently retained join",
                )
            })
    } else {
        queries.events(page.target, page.after, page.limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verification_argv_uses_absolute_daemon_context_and_reported_selection() {
        use std::path::Path;
        let context = VerificationContext::new(
            Path::new("daemon config/auth.yaml"),
            Path::new("relative daemon state"),
            Path::new("native claude"),
        )
        .expect("paths");
        let mut agent = crate::runtime::test_support::agent();
        agent.profile.provider = crate::config::Provider::Claude;
        agent.session_selection = Some(agent.desired_selection());
        agent.model = "newer-desired-model".into();
        let targets = verification_targets(&agent, Some(&context));
        let argv = targets[0].argv.as_ref().expect("daemon argv");
        assert_eq!(
            argv[2],
            std::path::absolute("daemon config/auth.yaml")
                .expect("absolute config")
                .to_str()
                .expect("utf8")
        );
        assert_eq!(
            argv[4],
            std::path::absolute("relative daemon state")
                .expect("absolute state")
                .to_str()
                .expect("utf8")
        );
        assert_eq!(
            argv[6],
            std::path::absolute("native claude")
                .expect("absolute binary")
                .to_str()
                .expect("utf8")
        );
        assert_eq!(argv[11], "gpt-5.6-luna");
        assert_eq!(
            targets[1].argv.as_ref().expect("next argv")[11],
            "newer-desired-model"
        );
    }
    #[test]
    fn verification_argv_never_replaces_non_utf8_path_bytes() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};
        assert!(
            VerificationContext::new(
                Path::new("/config"),
                Path::new("/state"),
                Path::new(OsStr::from_bytes(b"/native-\xff"))
            )
            .is_err()
        );
    }
    #[test]
    fn verification_targets_distinguish_current_session_and_next_selection() {
        let mut agent = crate::runtime::test_support::agent();
        agent.profile.provider = crate::config::Provider::Claude;
        agent.session_selection = Some(agent.desired_selection());
        agent.model = "new-model".into();
        let targets = verification_targets(&agent, None);
        assert_eq!(targets.len(), 2);
        assert!(targets[0].selection == api::VerificationSelection::CurrentSession);
        assert_eq!(targets[0].model, "gpt-5.6-luna");
        assert!(targets[1].selection == api::VerificationSelection::NextTurn);
        assert_eq!(targets[1].model, "new-model");
        agent.steering = crate::runtime::domain::SteeringCapability::Supported;
        assert!(verification_targets(&agent, None).is_empty());
    }
}
