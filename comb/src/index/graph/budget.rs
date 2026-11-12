use crate::{
    Error,
    graph::{Cancellation, Incomplete},
};
use std::{
    error::Error as _,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Stop {
    deadline: Instant,
    cancellation: Cancellation,
}
impl Stop {
    pub fn new(milliseconds: u64, cancellation: Cancellation) -> Self {
        Self {
            deadline: Instant::now() + Duration::from_millis(milliseconds),
            cancellation,
        }
    }
    pub fn reason(&self) -> Option<Incomplete> {
        if self.cancellation.is_cancelled() {
            Some(Incomplete::Cancelled)
        } else if Instant::now() >= self.deadline {
            Some(Incomplete::Deadline)
        } else {
            None
        }
    }
    pub fn interruption(&self, error: &Error) -> Option<Incomplete> {
        // A time limit must never disguise an unrelated database/corruption error.
        let sqlite = error.source()?.downcast_ref::<rusqlite::Error>()?;
        (sqlite.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted))
            .then(|| self.reason())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadline_interrupts_database_work_without_masking_storage_failures() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let stop = Stop::new(1, Cancellation::default());
        let handler = stop.clone();
        db.progress_handler(1000, Some(move || handler.reason().is_some()))
            .unwrap();
        let error = db.query_row("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n", [], |r| r.get::<_, i64>(0)).unwrap_err();
        assert_eq!(stop.interruption(&error.into()), Some(Incomplete::Deadline));
        db.progress_handler(0, None::<fn() -> bool>).unwrap();
        let error = db
            .query_row("SELECT absent FROM no_such_table", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_err();
        assert_eq!(stop.interruption(&error.into()), None);
    }
}
