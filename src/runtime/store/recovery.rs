//! Restart endings are uncertain history, never proof of native turn completion.
use super::{CHILD_PAGE_ROWS, Store, api, convert, rows};
use crate::{error::Result, runtime::domain::EventKind};
use rusqlite::params;

const UNFINISHED: &str = r"
SELECT a.listing_seq, a.id FROM agents a
WHERE a.listing_seq > ?1 AND a.execution_id IS NOT NULL
    AND (a.claimed = 1 OR a.state IN ('starting', 'idle', 'running', 'stopping'))
    AND EXISTS (SELECT 1 FROM events e WHERE e.agent = a.id
        AND e.execution = a.execution_id AND e.kind = 'execution_starting')
    AND NOT EXISTS (SELECT 1 FROM events e WHERE e.agent = a.id
        AND e.execution = a.execution_id
        AND e.kind IN ('execution_stopped', 'execution_failed'))
ORDER BY a.listing_seq LIMIT ?2
";

impl Store {
    pub(super) fn record_recovered_executions(&self) -> Result<()> {
        let mut after = 0;
        let limit = convert::stamp(crate::convert::usize_u64(CHILD_PAGE_ROWS))?;
        loop {
            let page: Vec<(u64, String)> = self.sql().cached().query_map(
                UNFINISHED,
                params![crate::convert::capped_i64(after), limit],
                |row| Ok((rows::unsigned(row, 0)?, row.get(1)?)),
            )?;
            if page.is_empty() {
                return Ok(());
            }
            for (sequence, id) in page {
                let agent = self.agent(&id)?;
                self.event_typed(
                    &agent,
                    EventKind::ExecutionStopped,
                    api::events::ExecutionStopped::Recovery {
                        reason: "daemon_restarted",
                        outcome: "uncertain",
                        turn_id: agent.turn_id.as_deref(),
                    },
                )?;
                after = sequence;
            }
        }
    }
}
