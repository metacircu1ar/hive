//! Owned service results are encoded canonically only by their transport adapter.
use crate::{Result, search::Results, store, values::Slug};
use comb_protocol::{import, service::api};
use serde::Serialize;

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Response {
    Capabilities(api::Capabilities),
    Health(api::Health),
    Document(Box<store::Document>),
    Search(Results),
    List(Vec<store::Summary>),
    Links(Vec<Slug>),
    References(Box<crate::links::Report>),
    Graph(Box<crate::graph::Report>),
    Stats(store::Stats),
    Doctor(Box<store::Doctor>),
    Mutation(store::Mutation),
    Consolidated(store::Consolidated),
    Removed(store::Removed),
    Import(import::ImportReport),
    ImportStatus(import::ImportStatus),
    ImportProgress(import::ImportProgress),
    Credential(super::super::auth::Credential),
    Credentials(Vec<super::super::auth::Listing>),
    Revoked(Revoked),
}

#[derive(Serialize)]
pub(crate) struct Revoked {
    pub revoked: crate::values::CredentialId,
}

impl Response {
    pub(crate) fn into_value(self) -> Result<serde_json::Value> {
        crate::error::value(self)
    }
}

macro_rules! response_from {
    ($($variant:ident($kind:ty)),+ $(,)?) => {
        $(impl From<$kind> for Response {
            fn from(reply: $kind) -> Self { Self::$variant(reply) }
        })+
    };
}
response_from! {
    Capabilities(api::Capabilities), Health(api::Health), Search(Results),
    List(Vec<store::Summary>), Links(Vec<Slug>), Stats(store::Stats),
    Mutation(store::Mutation), Consolidated(store::Consolidated), Removed(store::Removed),
    Import(import::ImportReport), ImportStatus(import::ImportStatus),
    ImportProgress(import::ImportProgress), Credential(super::super::auth::Credential),
    Credentials(Vec<super::super::auth::Listing>), Revoked(Revoked),
}
impl From<store::Document> for Response {
    fn from(reply: store::Document) -> Self {
        Self::Document(Box::new(reply))
    }
}
impl From<store::Doctor> for Response {
    fn from(reply: store::Doctor) -> Self {
        Self::Doctor(Box::new(reply))
    }
}
impl From<crate::graph::Report> for Response {
    fn from(reply: crate::graph::Report) -> Self {
        Self::Graph(Box::new(reply))
    }
}
impl From<crate::links::Report> for Response {
    fn from(reply: crate::links::Report) -> Self {
        Self::References(Box::new(reply))
    }
}
