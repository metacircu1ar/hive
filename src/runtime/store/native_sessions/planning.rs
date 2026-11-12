//! Commit exact cleanup identities with agent deletion, before touching files.
mod session;
use super::{files, preflight::Planner, queries};
use crate::{
    config::Provider,
    error::{Error, Result},
    runtime::{
        api,
        store::{Queries, Store},
    },
};
use rusqlite::params;
use session::{Disposition, Outcome, Preview};
use std::path::PathBuf;

super::super::rows::record! {
    pub(super) struct Binding {
        pub agent: String = 0,
        pub generation: String = 1,
        pub provider: String = 2,
        pub thread: String = 3,
    }
}

pub(super) fn invalid() -> Error {
    Error::database_error("Invalid native session cleanup record.")
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(data: &str) -> Result<T> {
    crate::decode::from_str(data).map_err(|cause| invalid().with_source(cause))
}
pub(super) fn provider(value: &str) -> Result<Provider> {
    match value {
        "codex" => Ok(Provider::Codex),
        "claude" => Ok(Provider::Claude),
        _ => Err(invalid()),
    }
}
pub(super) fn generation(value: &str) -> Result<uuid::Uuid> {
    let id = uuid::Uuid::parse_str(value).map_err(|cause| invalid().with_source(cause))?;
    if id.to_string() != value {
        return Err(invalid());
    }
    Ok(id)
}

impl Queries {
    pub(super) fn native_home(&self, generation: uuid::Uuid) -> Result<PathBuf> {
        Ok(self
            .native_root
            .as_ref()
            .ok_or_else(invalid)?
            .join("profiles")
            .join(generation.to_string())
            .join("provider"))
    }
    pub(in crate::runtime) fn native_cleanup_receipt(
        &self,
        request: &str,
    ) -> Result<Option<api::NativeCleanup>> {
        let data: Option<String> =
            self.query_opt_cached(queries::READ_JOB, [request], |r| r.get(0))?;
        data.map(|data| decode(&data)).transpose()
    }
}

impl Store {
    pub(in crate::runtime::store) fn plan_native_cleanup(
        &self,
        request: &str,
        options: api::CleanupOptions,
        planner: &mut Planner,
    ) -> Result<api::NativeCleanup> {
        let api::CleanupOptions {
            dry_run,
            keep_native_history: keep,
        } = options;
        let context = CleanupRequest {
            id: request,
            options,
        };
        let mut report = api::NativeCleanup::new(keep || dry_run);
        if !dry_run {
            self.execute_cached(
                queries::INSERT_JOB,
                params![request, crate::api::json_text(&report)],
            )?;
        }
        let mut cursor = (String::new(), String::new(), String::new());
        loop {
            let bindings: Vec<Binding> = self.sql().cached().query_rows(
                queries::SCOPE_SESSIONS,
                params![cursor.0, cursor.1, cursor.2],
            )?;
            if bindings.is_empty() {
                break;
            }
            for binding in bindings {
                let outcome = self.plan_native_session(
                    context,
                    &binding,
                    planner,
                    report.native_preview.len() >= api::NATIVE_PREVIEW_LIMIT,
                )?;
                report.record_session(&binding.agent, outcome);
                cursor = (binding.generation, binding.provider, binding.thread);
            }
        }
        if !keep {
            let mut after = String::new();
            loop {
                let agents: Vec<String> = self
                    .sql()
                    .cached()
                    .query_column(queries::UNKNOWN_SCOPE, [&after])?;
                if agents.is_empty() {
                    break;
                }
                for agent in agents {
                    report.warn(&agent, api::NativeWarningReason::IncompleteHistory);
                    after = agent;
                }
            }
        }
        if !dry_run {
            self.save_native_cleanup_receipt(request, &report)?;
        }
        Ok(report)
    }
    pub(super) fn save_native_cleanup_receipt(
        &self,
        request: &str,
        report: &api::NativeCleanup,
    ) -> Result<()> {
        self.execute_cached(
            queries::SAVE_JOB,
            params![request, crate::api::json_text(report)],
        )?;
        Ok(())
    }
    fn plan_native_session(
        &self,
        request: CleanupRequest<'_>,
        binding: &Binding,
        planner: &mut Planner,
        preview_full: bool,
    ) -> Result<Outcome> {
        let api::CleanupOptions {
            dry_run,
            keep_native_history: keep,
        } = request.options;
        let generation = generation(&binding.generation)?;
        let provider = provider(&binding.provider)?;
        let shared: bool = self.query_row_cached(
            queries::SURVIVING_OWNER,
            params![binding.generation, binding.provider, binding.thread],
            |r| r.get(0),
        )?;
        if shared && !dry_run {
            self.execute_cached(
                queries::PRESERVE_LOCATIONS,
                params![binding.generation, binding.provider, binding.thread],
            )?;
        }
        if keep {
            return Ok(Outcome::retained(None));
        }
        if shared {
            return Ok(Outcome::retained(Some(
                api::NativeWarningReason::SharedSession,
            )));
        }
        let plan = planner.plan(self, binding)?;
        let preview = if dry_run {
            session::preview(binding, provider, generation, &plan.files, preview_full)
        } else {
            Preview::NotRequested
        };
        let disposition = if plan.warning.is_some() || dry_run {
            Disposition::Retained
        } else if plan.files.is_empty() {
            Disposition::Absent
        } else {
            self.queue_native_files(request.id, binding, &plan.files)?;
            Disposition::Queued
        };
        Ok(Outcome {
            disposition,
            warning: plan.warning,
            preview,
        })
    }
    fn queue_native_files(
        &self,
        request: &str,
        binding: &Binding,
        files: &[files::File],
    ) -> Result<()> {
        let session: i64 = self.query_row_cached(
            queries::INSERT_SESSION,
            params![
                request,
                binding.agent,
                binding.generation,
                binding.provider,
                binding.thread,
                i64::try_from(files.len()).map_err(super::super::numeric)?
            ],
            |r| r.get(0),
        )?;
        for file in files {
            self.execute_cached(
                queries::INSERT_FILE,
                params![session, crate::api::json_text(file)],
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct CleanupRequest<'a> {
    id: &'a str,
    options: api::CleanupOptions,
}
