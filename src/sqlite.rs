//! SQLite execution helpers preserve statement caching and domain error context.
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Params, Row, Statement, types::FromSql};

pub(crate) trait FromRow: Sized {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self>;
}

pub(crate) fn db(cause: rusqlite::Error) -> Error {
    Error::database_error("Cannot access the runtime database.").with_source(cause)
}

/// Connection setup also uses the runtime database error classification.
pub(crate) trait RuntimeResult<T> {
    fn runtime(self) -> Result<T>;
}
impl<T> RuntimeResult<T> for rusqlite::Result<T> {
    fn runtime(self) -> Result<T> {
        self.map_err(db)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Database<'a> {
    connection: &'a Connection,
    error: fn(rusqlite::Error) -> Error,
    cached: bool,
    poison: Option<&'a std::cell::RefCell<Option<Error>>>,
}
impl<'a> Database<'a> {
    pub fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            error: db,
            cached: false,
            poison: None,
        }
    }
    pub(crate) fn healthy(self) -> Result<()> {
        match self.poison.and_then(|poison| poison.borrow().clone()) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub(crate) fn with_poison(mut self, poison: &'a std::cell::RefCell<Option<Error>>) -> Self {
        self.poison = Some(poison);
        self
    }
    pub fn map_error(self, error: fn(rusqlite::Error) -> Error) -> Self {
        Self { error, ..self }
    }
    pub fn cached(self) -> Self {
        Self {
            cached: true,
            ..self
        }
    }
    pub fn execute(self, sql: &str, params: impl Params) -> Result<usize> {
        self.statement(sql, |statement| {
            statement.execute(params).map_err(self.error)
        })
    }
    pub fn execute_batch(self, sql: &str) -> Result<()> {
        self.healthy()?;
        self.connection.execute_batch(sql).map_err(self.error)
    }
    pub fn query_row<T>(
        self,
        sql: &str,
        params: impl Params,
        row: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.statement(sql, |statement| {
            statement.query_row(params, row).map_err(self.error)
        })
    }
    pub fn query_opt<T>(
        self,
        sql: &str,
        params: impl Params,
        row: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        self.statement(sql, |statement| {
            statement
                .query_row(params, row)
                .optional()
                .map_err(self.error)
        })
    }
    pub fn query_rows<T: FromRow>(self, sql: &str, params: impl Params) -> Result<Vec<T>> {
        self.query_map(sql, params, T::read)
    }
    pub fn query_column<T: FromSql>(self, sql: &str, params: impl Params) -> Result<Vec<T>> {
        self.query_map(sql, params, |row| row.get(0))
    }
    pub fn query_map<T>(
        self,
        sql: &str,
        params: impl Params,
        row: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        self.statement(sql, |statement| {
            statement
                .query_map(params, row)
                .and_then(Iterator::collect)
                .map_err(self.error)
        })
    }
    pub fn with_rows<T>(
        self,
        sql: &str,
        params: impl Params,
        read: impl FnOnce(&mut Cursor<'_>) -> Result<T>,
    ) -> Result<T> {
        self.statement(sql, |statement| {
            let rows = statement.query(params).map_err(self.error)?;
            read(&mut Cursor {
                rows,
                error: self.error,
            })
        })
    }
    fn statement<T>(
        self,
        sql: &str,
        read: impl FnOnce(&mut Statement<'_>) -> Result<T>,
    ) -> Result<T> {
        self.healthy()?;
        if self.cached {
            let mut statement = self.connection.prepare_cached(sql).map_err(self.error)?;
            read(&mut statement)
        } else {
            let mut statement = self.connection.prepare(sql).map_err(self.error)?;
            read(&mut statement)
        }
    }
}

/// Reads only the requested rows; page limits must not decode later records.
pub(crate) struct Cursor<'a> {
    rows: rusqlite::Rows<'a>,
    error: fn(rusqlite::Error) -> Error,
}
impl Cursor<'_> {
    pub fn next<T: FromRow>(&mut self) -> Result<Option<T>> {
        self.next_with(T::read)
    }
    pub fn column<T: FromSql>(&mut self) -> Result<Option<T>> {
        self.next_with(|row| row.get(0))
    }
    pub fn next_with<T>(
        &mut self,
        row: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        self.rows
            .next()
            .and_then(|value| value.map(row).transpose())
            .map_err(self.error)
    }
}

pub(crate) fn cleanup_failure(phase: &'static str, primary: &Error, secondary: &Error) {
    // Keep the diagnostic namespace independent of the helper module location.
    tracing::error!(name: "store.cleanup_failed", target: "hive::runtime::store::sql",
         phase = phase,
         code = primary.code(),
         primary = primary as &(dyn std::error::Error + 'static),
         secondary = secondary as &(dyn std::error::Error + 'static),
         sqlite_code = sqlite_code(secondary),
         decision = "discard_connection");
}

/// Only native numeric codes cross the privacy boundary; SQLite text can contain SQL.
pub(crate) fn sqlite_code(error: &(dyn std::error::Error + 'static)) -> i32 {
    let mut cause = Some(error);
    for _ in 0..8 {
        let Some(current) = cause else { break };
        if let Some(rusqlite::Error::SqliteFailure(code, _)) =
            current.downcast_ref::<rusqlite::Error>()
        {
            return code.extended_code;
        }
        cause = current.source();
    }
    0
}

// Keep each query's column order, named fields and checked decoders together.
macro_rules! record {
    ($(#[$meta:meta])* $vis:vis struct $name:ident {
        $($field_vis:vis $field:ident: $ty:ty = $index:literal $(=> $decode:path)?),* $(,)?
    }) => {
        $(#[$meta])*
        $vis struct $name { $($field_vis $field: $ty),* }
        impl $crate::sqlite::FromRow for $name {
            fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
                Ok(Self { $($field: $crate::sqlite::record!(@read row,
                     $index $(,
                     $decode)?)?),
                    * })
            }
        }
    };
    (@read $row:ident, $index:literal, $decode:path) => { $decode($row, $index) };
    (@read $row:ident, $index:literal) => { $row.get($index) };
}
pub(crate) use record;
