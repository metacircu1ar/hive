//! State-owned runtime format checks and same-process SQLite file admission.
pub(crate) mod migration;
pub(crate) mod schema;
use crate::{
    error::{Error, Result},
    locks::MutexExt as _,
    sqlite::RuntimeResult as _,
    state,
};
use rusqlite::Connection;
use std::path::Path;

static ADMISSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Exclude raw publication descriptor closes from same-process SQLite admission.
pub(crate) fn database_admission() -> std::sync::MutexGuard<'static, ()> {
    ADMISSION.lock_or_panic()
}
/// Inspect only schema pages through SQLite; never copy history or open raw DB fds.
pub(crate) fn check_existing_format(path: &Path) -> Result<()> {
    if !check_files(path)? {
        return Ok(());
    }
    let connection = read_connection(path)?;
    schema::validate(&connection)
}
/// Profile operations need cross-file identity agreement, without assembling a
/// reference database on every metadata transaction. Runtime opens validate all SQL.
pub(crate) fn check_existing_identity(path: &Path) -> Result<()> {
    if !check_files(path)? {
        return Ok(());
    }
    schema::identity(&read_connection(path)?)
}
pub(crate) fn check_files(path: &Path) -> Result<bool> {
    let parent = path
        .parent()
        .ok_or(Error::invalid_path("Missing database directory."))?;
    match parent.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::state_io_from(error)),
        Ok(metadata) => state::check_metadata(&metadata, true)?,
    }
    // Raw opens/closes would release another SQLite connection's process-owned
    // fcntl locks. Metadata checks do not disturb SQLite's descriptor bookkeeping.
    let (present, populated) = match path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, false),
        Err(error) => return Err(Error::state_io_from(error)),
        Ok(metadata) => {
            state::check_metadata(&metadata, false)?;
            (true, metadata.len() != 0)
        }
    };
    for suffix in ["-wal", "-journal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = Path::new(&sidecar);
        let metadata = match sidecar.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(Error::state_io_from(error)),
            Ok(metadata) => metadata,
        };
        if !populated {
            return Err(Error::recovery_required(concat!(
                "Runtime database is missing or empty but sidecar evidence remains; ",
                "use a fresh state directory and retain this history.",
            )));
        }
        state::check_metadata(&metadata, false)?;
        if suffix == "-journal" && metadata.len() != 0 {
            return Err(Error::recovery_required(concat!(
                "Runtime history has an unresolved rollback journal; retain it and ",
                "use a fresh state directory.",
            )));
        }
    }
    Ok(present)
}
#[cfg(test)]
thread_local! { static READ_CONNECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn read_connection_count() -> usize {
    READ_CONNECTIONS.get()
}
pub(crate) fn read_connection(path: &Path) -> Result<Connection> {
    #[cfg(test)]
    READ_CONNECTIONS.set(READ_CONNECTIONS.get() + 1);
    let _admission = database_admission();
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).runtime()?;
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .runtime()?;
    Ok(connection)
}

#[cfg(test)]
pub(crate) fn try_database_admission()
-> std::sync::TryLockResult<std::sync::MutexGuard<'static, ()>> {
    ADMISSION.try_lock()
}
