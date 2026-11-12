//! Validate owned runtime files and open writer or read-only SQLite connections.
mod queries;
use super::{Store, schema, sql::RuntimeResult as _};
use crate::{
    error::{Error, Result},
    state,
};
use rusqlite::Connection;
mod creation;
use crate::state::database::{check_files, database_admission, read_connection};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};
// Control reads tolerate only a short SQLite busy wait, independently of writer work.
const READ_BUSY_TIMEOUT: Duration = Duration::from_millis(50);
pub struct Queries {
    pub(super) connection: Connection,
    pub(super) poison: RefCell<Option<Error>>,
    pub(super) artifacts_dir: PathBuf,
    pub(super) native_root: Option<PathBuf>,
    pub(in crate::runtime) attachments_dir: PathBuf,
    #[cfg(test)]
    pub(super) test_hooks: crate::runtime::test_support::TestHooks,
}
impl Queries {
    #[cfg(test)]
    pub(in crate::runtime) fn test_connection(&self) -> &Connection {
        &self.connection
    }
    #[cfg(test)]
    pub(crate) fn test_hooks(&self) -> &crate::runtime::test_support::TestHooks {
        &self.test_hooks
    }
    pub(in crate::runtime) fn open_read_only(path: &Path) -> Result<Self> {
        if !check_files(path)? {
            return Err(Error::database_error("Runtime database is missing."));
        }
        let connection = read_connection(path)?;
        // Bound each SQLite busy wait on the reader only; writer timeout stays zero.
        connection.busy_timeout(READ_BUSY_TIMEOUT).runtime()?;
        connection
            .pragma_update(None, "query_only", true)
            .runtime()?;
        schema::validate(&connection)?;
        connection.set_prepared_statement_cache_capacity(super::STATEMENT_CACHE_CAPACITY);
        let artifacts_dir = output_directory(path)?;
        state::check_path(&artifacts_dir, true)?;
        state::check_path(&path.with_file_name("attachments"), true)?;
        Ok(Self {
            connection,
            poison: RefCell::default(),
            attachments_dir: path
                .with_file_name("attachments")
                .canonicalize()
                .map_err(Error::state_io_from)?,
            artifacts_dir,
            native_root: native_root(path),
            #[cfg(test)]
            test_hooks: crate::runtime::test_support::TestHooks::default(),
        })
    }
}

impl Store {
    pub(in crate::runtime) fn bind_native_root(&mut self, root: PathBuf) {
        self.queries.native_root = Some(root);
    }
    pub fn open(path: &Path) -> Result<Self> {
        let conn = creation::open(path)?;
        let outputs = output_directory(path)?;
        state::directory(&outputs)?;
        let attachments_dir = path.with_file_name("attachments");
        state::directory(&attachments_dir)?;
        let attachments_dir = attachments_dir
            .canonicalize()
            .map_err(Error::state_io_from)?;
        let joins_possible = conn
            .query_row(queries::OPEN_SELECT, [], |r| r.get(0))
            .runtime()?;
        Ok(Self {
            #[cfg(feature = "comb")]
            collectors_possible: Cell::new(
                conn.query_row(queries::OPEN_SELECT_2, [], |r| r.get(0))
                    .runtime()?,
            ),
            joins_possible: Cell::new(joins_possible),
            queries: Queries {
                connection: conn,
                poison: RefCell::default(),
                artifacts_dir: outputs,
                native_root: native_root(path),
                attachments_dir,
                #[cfg(test)]
                test_hooks: crate::runtime::test_support::TestHooks::default(),
            },
            transaction_depth: Cell::new(0),
            notices: RefCell::default(),
            pending_wakes: RefCell::new(HashSet::new()),
            wake_all: Cell::new(false),
            deadlines_changed: Cell::new(false),
            observed: RefCell::new(HashSet::new()),
            observed_all: Cell::new(false),
            tree_changed: Cell::new(false),
        })
    }
}

/// The background checkpointer's connection. Read-write, because checkpointing
/// writes the database file, and `synchronous = FULL`, because the checkpointing
/// connection's setting governs that file's sync. It never closes last, so it
/// leaves the close-time checkpoint to the writer's connection.
pub(super) fn checkpoint_connection(path: &Path) -> Result<Connection> {
    let _admission = database_admission();
    let flags = rusqlite::OpenFlags::default() & !rusqlite::OpenFlags::SQLITE_OPEN_CREATE;
    let connection = Connection::open_with_flags(path, flags).runtime()?;
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .runtime()?;
    connection
        .busy_timeout(std::time::Duration::ZERO)
        .runtime()?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .runtime()?;
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .runtime()?;
    Ok(connection)
}
fn output_directory(path: &Path) -> Result<PathBuf> {
    Ok(path
        .parent()
        .ok_or_else(|| Error::database_error("Missing database directory."))?
        .join("output"))
}

fn native_root(path: &Path) -> Option<PathBuf> {
    let runtime = path.parent()?;
    (runtime.file_name()? == "runtime")
        .then(|| runtime.parent().map(Path::to_path_buf))
        .flatten()
}
