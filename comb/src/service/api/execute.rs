//! Typed snapshot and administrative execution after admission.
use super::{
    Capabilities,
    actions::{AdminAction, ReadAction},
};
use super::{Response, Revoked};
use crate::{
    Comb, Result,
    read::Snapshot,
    search::QueryVector,
    service::auth::{Authority, Registry},
};

pub(crate) trait ReadExecution {
    #[expect(
        clippy::too_many_arguments,
        reason = "Read execution needs snapshot, query, model status, cancellation and authority"
    )]
    fn execute(
        self,
        snapshot: &Snapshot,
        query: QueryVector,
        runtime: Option<crate::embedding::RuntimeInfo>,
        cancellation: crate::graph::Cancellation,
        authority: &Authority,
    ) -> Result<Response>;
}
impl ReadExecution for ReadAction<'_> {
    fn execute(
        self,
        snapshot: &Snapshot,
        query: QueryVector,
        runtime: Option<crate::embedding::RuntimeInfo>,
        cancellation: crate::graph::Cancellation,
        authority: &Authority,
    ) -> Result<Response> {
        match self {
            Self::Capabilities => Ok(Response::from(Capabilities {
                protocols: comb_protocol::compatibility::SERVICE_RANGE,
                reviewer: authority.reviewer,
            })),
            Self::Health => Ok(Response::from(health(snapshot)?)),
            Self::Get(slug) => Ok(Response::from(snapshot.index.document(slug)?)),
            Self::Search(request, mode) => Ok(Response::from(
                snapshot
                    .index
                    .retrieve_cancellable(request, mode, query, cancellation)?,
            )),
            Self::List(request) => Ok(Response::from(snapshot.index.list(request)?)),
            Self::Links(slug, incoming, after, limit) => Ok(Response::from(
                snapshot.index.link_report(slug, incoming, after, limit)?,
            )),
            Self::Graph(slug, depth) => Ok(Response::from(snapshot.index.graph(slug, depth)?)),
            Self::GraphQuery(request) => Ok(Response::from(
                snapshot.index.graph_query(request, cancellation)?,
            )),
            Self::Stats => {
                let mut stats = snapshot.index.stats()?;
                stats.embedding_runtime = runtime;
                Ok(Response::from(stats))
            }
        }
    }
}

pub(crate) struct PreparedAdmin<'a>(AdminAction<'a>);
pub(crate) trait AdminPreparation<'a> {
    fn prepare(self, authority: &Authority) -> Result<PreparedAdmin<'a>>;
}
impl<'a> AdminPreparation<'a> for AdminAction<'a> {
    fn prepare(self, authority: &Authority) -> Result<PreparedAdmin<'a>> {
        authority.permit(self.required())?;
        Ok(PreparedAdmin(self))
    }
}
impl PreparedAdmin<'_> {
    pub(crate) fn execute(self, comb: &mut Comb, registry: &Registry) -> Result<Response> {
        match self.0 {
            AdminAction::Doctor => Ok(Response::from(comb.doctor()?)),
            AdminAction::Sync => Ok(Response::from(comb.sync()?)),
            AdminAction::Import(directory, prefix) => {
                Ok(Response::from(comb.import(directory, prefix)?))
            }
            AdminAction::ImportStatus => Ok(Response::from(comb.import_status()?)),
            AdminAction::ImportAbandon(batch_id) => {
                Ok(Response::from(comb.abandon_import(batch_id)?))
            }
            AdminAction::CredentialCreate(grant) => {
                Ok(Response::from(registry.create_input(grant.clone())?))
            }
            AdminAction::CredentialList => Ok(Response::from(registry.list()?)),
            AdminAction::CredentialRevoke(id) => {
                registry.revoke(id)?;
                Ok(Response::from(Revoked {
                    revoked: id.clone(),
                }))
            }
        }
    }
}

pub(in crate::service) fn health(
    snapshot: &Snapshot,
) -> Result<comb_protocol::service::api::Health> {
    use comb_protocol::service::api::{Health, ModelHealth};
    let coverage = snapshot
        .index
        .embedding_coverage(crate::search::Filter::default())?;
    let build = crate::build_info();
    Ok(Health {
        managed_service: hive_setup_support::managed::label().map(str::to_owned),
        product_version: build.product_version.into(),
        compiled_features: build
            .compiled_features
            .iter()
            .map(|s| (*s).into())
            .collect(),
        model: snapshot
            .index
            .active_space()?
            .map(|(lease, space)| ModelHealth {
                space_id: lease.fingerprint.to_string(),
                name: space.model,
                revision: space.revision,
                backend: space.backend,
                dimensions: space.dimensions,
                query_ready: false,
            }),
        total: coverage.total,
        embedded: coverage.embedded,
        pending: coverage.pending,
        rejected: coverage.rejected,
    })
}
