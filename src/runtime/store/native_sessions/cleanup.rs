//! Bounded post-commit unlinks and replay-safe acknowledgement of cleanup totals.
use super::super::sql::FromRow as _;
use super::{files, planning, queries};
use crate::{
    config::Provider,
    error::Result,
    runtime::{
        api,
        store::{Queries, Store},
    },
};
use rusqlite::params;
use std::path::PathBuf;

super::super::rows::record! {
    struct Pending {
        id: i64 = 0 => super::super::rows::nonnegative,
        generation: String = 1,
        provider: String = 2,
        thread: String = 3,
        data: String = 4,
        shared: bool = 5,
    }
}
super::super::rows::record! {
    struct Session {
        id: i64 = 0 => super::super::rows::nonnegative,
        request: String = 1,
        agent: String = 2,
        remaining: i64 = 3 => super::super::rows::nonnegative,
        deleted: i64 = 4 => super::super::rows::nonnegative,
        absent: i64 = 5 => super::super::rows::nonnegative,
        retained: i64 = 6 => super::super::rows::nonnegative,
        bytes: i64 = 7 => super::super::rows::nonnegative,
        warning: Option<String> = 8,
    }
}
struct Task {
    id: i64,
    home: PathBuf,
    provider: Provider,
    thread: String,
    file: files::File,
    shared: bool,
}
pub(in crate::runtime) struct NativeDeletions(Vec<Task>);
impl NativeDeletions {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
pub(in crate::runtime) struct NativeDeleted(Vec<(i64, files::Removed)>);

pub(in crate::runtime) fn unlink_native_files(
    batch: NativeDeletions,
) -> (NativeDeleted, Result<()>) {
    let mut outcomes = Vec::with_capacity(batch.0.len());
    for task in batch.0 {
        let outcome = if task.shared {
            files::Removed::Retained(api::NativeWarningReason::SharedSession)
        } else {
            match files::unlink(&task.home, task.provider, &task.thread, &task.file) {
                Ok(outcome) => outcome,
                Err(error) => return (NativeDeleted(outcomes), Err(error)),
            }
        };
        outcomes.push((task.id, outcome));
    }
    (NativeDeleted(outcomes), Ok(()))
}

impl Queries {
    pub(in crate::runtime) fn pending_native_deletions(&self) -> Result<NativeDeletions> {
        let rows: Vec<Pending> = self.sql().cached().query_rows(queries::PENDING_FILES, [])?;
        let mut tasks = Vec::with_capacity(rows.len());
        for row in rows {
            tasks.push(Task {
                id: row.id,
                home: self.native_home(planning::generation(&row.generation)?)?,
                provider: planning::provider(&row.provider)?,
                thread: row.thread,
                shared: row.shared,
                file: planning::decode(&row.data)?,
            });
        }
        if tasks.is_empty() && self.has_native_deletions()? {
            return Err(planning::invalid());
        }
        Ok(NativeDeletions(tasks))
    }
    pub(in crate::runtime) fn has_native_deletions(&self) -> Result<bool> {
        self.query_row_cached(queries::HAS_PENDING, [], |r| r.get(0))
    }
    pub(in crate::runtime) fn native_cleanup_pending(&self, request: &str) -> Result<bool> {
        self.query_row_cached(queries::REQUEST_PENDING, [request], |r| r.get(0))
    }
}
impl Store {
    pub(in crate::runtime) fn acknowledge_native_deletions(
        &self,
        deleted: NativeDeleted,
    ) -> Result<()> {
        for (id, outcome) in deleted.0 {
            let Some(mut session) =
                self.sql()
                    .cached()
                    .query_opt(queries::FILE_SESSION, [id], Session::read)?
            else {
                // A concurrent request may already have acknowledged this exact file.
                continue;
            };
            match outcome {
                files::Removed::Deleted(bytes) => {
                    session.deleted += 1;
                    session.bytes = session
                        .bytes
                        .checked_add(i64::try_from(bytes).map_err(super::super::numeric)?)
                        .ok_or_else(super::super::numeric_range)?;
                }
                files::Removed::AlreadyAbsent => session.absent += 1,
                files::Removed::Retained(reason) => {
                    session.retained += 1;
                    session.warning = Some(crate::api::json_text(reason));
                }
            }
            self.execute_cached(queries::DELETE_FILE, [id])?;
            match session.remaining {
                1 => self.finish_native_session(&session)?,
                2.. => {
                    self.execute_cached(
                        queries::SAVE_SESSION,
                        params![
                            session.id,
                            session.remaining - 1,
                            session.deleted,
                            session.absent,
                            session.retained,
                            session.bytes,
                            session.warning
                        ],
                    )?;
                }
                _ => return Err(planning::invalid()),
            }
        }
        Ok(())
    }
    fn finish_native_session(&self, session: &Session) -> Result<()> {
        let mut report = self
            .native_cleanup_receipt(&session.request)?
            .ok_or_else(planning::invalid)?;
        if session.retained > 0 {
            report.native_sessions_retained += 1;
            let warning: api::NativeWarningReason =
                serde_json::from_str(session.warning.as_deref().ok_or_else(planning::invalid)?)
                    .map_err(|cause| planning::invalid().with_source(cause))?;
            report.warn(&session.agent, warning);
        } else if session.deleted > 0 {
            report.native_sessions_deleted += 1;
        } else {
            report.native_sessions_absent += 1;
        }
        report.native_bytes_deleted = report
            .native_bytes_deleted
            .checked_add(session.bytes.cast_unsigned())
            .ok_or_else(super::super::numeric_range)?;
        self.save_native_cleanup_receipt(&session.request, &report)?;
        self.execute_cached(queries::DELETE_SESSION, [session.id])?;
        Ok(())
    }
    pub(in crate::runtime) fn finish_native_deletions(&self) -> Result<()> {
        loop {
            let batch = self.pending_native_deletions()?;
            if batch.is_empty() {
                return Ok(());
            }
            let (outcomes, result) = unlink_native_files(batch);
            let acknowledged = self.atomic(|store| store.acknowledge_native_deletions(outcomes));
            crate::observability::settle(result, acknowledged, "native_cleanup_ack")?;
        }
    }
}
