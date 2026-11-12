//! Runtime transactions, checkpoints, poison ownership and writer diagnostics.
use crate::error::{Error, Result};
pub(in crate::runtime) use crate::sqlite::sqlite_code;
pub(super) use crate::sqlite::{Cursor, Database, FromRow, RuntimeResult, cleanup_failure};

use super::{Queries, Store, queries};

impl Queries {
    pub(in crate::runtime) fn snapshot<T>(
        &self,
        read: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        self.sql().execute_batch(queries::SNAPSHOT)?;
        let result = read(self);
        // ROLLBACK closes a read-only snapshot on every Result path, without writes.
        let closed = self.close_snapshot();
        if let Err(error) = &closed {
            self.poison(error);
            cleanup_failure("snapshot", result.as_ref().err().unwrap_or(error), error);
        }
        match result {
            Ok(value) => closed.map(|()| value),
            Err(error) => Err(error),
        }
    }
    fn close_snapshot(&self) -> Result<()> {
        #[cfg(test)]
        self.test_hooks
            .sql("snapshot_close", &self.connection)
            .runtime()?;
        self.sql().execute_batch(queries::SNAPSHOT_2)
    }
    pub(in crate::runtime) fn healthy(&self) -> Result<()> {
        self.sql().healthy()
    }
    fn poison(&self, error: &Error) {
        let mut poison = self.poison.borrow_mut();
        if poison.is_none() {
            *poison = Some(error.clone());
        }
    }
    pub(in crate::runtime) fn autocommit(&self) -> bool {
        self.connection.is_autocommit()
    }
    pub(super) fn sql(&self) -> super::sql::Database<'_> {
        Database::new(&self.connection).with_poison(&self.poison)
    }
    pub(super) fn query_opt_cached<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        row: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        self.sql().cached().query_opt(sql, params, row)
    }
    pub(super) fn query_row_cached<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        row: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.sql().cached().query_row(sql, params, row)
    }
}

impl Store {
    pub(crate) fn outer_transaction(&self) -> bool {
        self.transaction_depth.get() == 1
    }
    /// Live writer only: savepoint undo pages need not spill to temporary files.
    /// Call after recovery and outside a transaction. Offline compaction keeps
    /// disk-backed temporaries because its frontier can span the entire history.
    pub(in crate::runtime) fn use_memory_temporaries(&self) -> Result<()> {
        self.healthy()?;
        self.connection
            .pragma_update(None, "temp_store", "MEMORY")
            .runtime()
    }
    pub(super) fn execute_cached(&self, sql: &str, params: impl rusqlite::Params) -> Result<usize> {
        self.sql().cached().execute(sql, params)
    }
    /// Nested savepoints retain the existing wake superset until the writer commits.
    pub(in crate::runtime) fn atomic<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        self.begin()?;
        let result = operation(self).and_then(|value| self.commit().map(|()| value));
        if let Err(error) = &result {
            self.rollback_after(error);
        }
        result
    }
    pub fn begin(&self) -> Result<()> {
        let depth = self.transaction_depth.get();
        #[cfg(test)]
        if depth == 0 {
            self.test_hooks.sql("begin", &self.connection).runtime()?;
        }
        self.execute_cached(&queries::begin_savepoint(depth), [])?;
        self.notices.borrow_mut().begin();
        self.transaction_depth.set(depth + 1);
        Ok(())
    }
    pub fn commit(&self) -> Result<()> {
        let depth = self
            .transaction_depth
            .get()
            .checked_sub(1)
            .ok_or(Error::database_error("No transaction."))?;
        if depth == 0 {
            #[cfg(test)]
            self.test_hooks.sql("commit", &self.connection).runtime()?;
            self.settle_task_outcomes()?;
            self.settle_joins()?;
        }
        self.execute_cached(&queries::release_savepoint(depth), [])?;
        self.transaction_depth.set(depth);
        self.notices.borrow_mut().commit();
        Ok(())
    }
    pub fn rollback(&self) -> Result<()> {
        self.healthy()?;
        let depth = self
            .transaction_depth
            .get()
            .checked_sub(1)
            .ok_or_else(|| Error::database_error("No transaction."))?;
        match self
            .sql()
            .execute_batch(&queries::rollback_savepoint(depth))
        {
            Ok(()) => {
                self.transaction_depth.set(depth);
                self.notices.borrow_mut().rollback();
                Ok(())
            }
            Err(error) => {
                self.notices.borrow_mut().discard();
                self.poison(&error);
                Err(error)
            }
        }
    }
    pub(in crate::runtime) fn rollback_after(&self, primary: &Error) {
        if let Err(secondary) = self.rollback() {
            cleanup_failure("rollback", primary, &secondary);
        }
    }
    /// PASSIVE, then TRUNCATE when no reader blocked it. Returns whether the WAL
    /// was fully copied, so that the next write restarts it.
    pub fn checkpoint(&self) -> Result<bool> {
        let timer = crate::observability::timing::Timer::start();
        let result = self.checkpoint_inner(true);
        timer.finish("application_checkpoint", result.is_ok());
        result
    }
    /// The writer's deferred checkpoint: PASSIVE only, so the WAL file keeps its size.
    pub fn checkpoint_passive(&self) -> Result<bool> {
        let timer = crate::observability::timing::Timer::start();
        let result = self.checkpoint_inner(false);
        timer.finish("deferred_checkpoint", result.is_ok());
        result
    }
    fn checkpoint_inner(&self, shrink: bool) -> Result<bool> {
        #[cfg(test)]
        self.test_hooks.checkpoint();
        #[cfg(test)]
        self.test_hooks
            .sql("checkpoint_passive", &self.connection)
            .runtime()?;
        let (busy, log, copied): (i64, i64, i64) =
            self.sql().query_row(queries::CHECKPOINT_PASSIVE, [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        if busy != 0 {
            tracing::debug!(name: "store.checkpoint_deferred",
                 phase = "passive",
                 decision = "defer");
            return Ok(false);
        }
        if !shrink {
            // A reader's snapshot can pin frames; the rest is copied next time.
            return Ok(log == copied);
        }
        // busy_timeout remains zero: a reader can never block the writer here.
        #[cfg(test)]
        self.test_hooks
            .sql("checkpoint_truncate", &self.connection)
            .runtime()?;
        let busy: i64 = self
            .sql()
            .query_row(queries::CHECKPOINT_TRUNCATE, [], |r| r.get(0))?;
        if busy != 0 {
            tracing::debug!(name: "store.checkpoint_deferred",
                 phase = "truncate",
                 decision = "defer");
            return Ok(log == copied);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
