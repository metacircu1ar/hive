//! Durable snapshots, approval details and child discovery for one follower.
use super::{
    AttachMetadata, Detail, ErrorCode, Follower, HISTORY_ROWS, IDLE_REFRESH_DELAY, Listing,
    MAX_BACKOFF, Observation, Ordering, Outcome, Update, Value, observe, problem, views,
};

pub(super) struct SnapshotProgress {
    pub(super) events_empty: bool,
    pub(super) approval: Option<String>,
    pub(super) children: u64,
}

impl Follower {
    /// Apply forward pages until one is empty; adopt the live cursor when asked.
    pub(super) fn refresh(&mut self, adopt_live: bool) -> bool {
        let mut adopt = adopt_live;
        while !self.cancelled.load(Ordering::Acquire) {
            let request = observe::value(Observation::Attach {
                target: &self.target,
                before: None,
                after: Some(self.durable),
                limit: Some(HISTORY_ROWS),
            });
            match self.read(&request) {
                Outcome::Ok(reply) => {
                    if !self.slot(false) {
                        return false;
                    }
                    let progress = self.adopt(&reply, adopt);
                    adopt = false;
                    let empty = progress.events_empty;
                    if !self.send(Update::Refresh(reply)) || !self.after_reply(progress) {
                        return false;
                    }
                    if empty {
                        return true;
                    }
                }
                Outcome::Failed {
                    code: ErrorCode::UnknownAgent,
                    ..
                } => {
                    self.send(Update::Purged);
                    return false;
                }
                Outcome::Failed {
                    code: ErrorCode::WatchCapacity,
                    ..
                } => {
                    if !self.slot(true) {
                        return false;
                    }
                    (self.pause)(IDLE_REFRESH_DELAY);
                }
                failure => {
                    if !self.send(problem(failure)) {
                        return false;
                    }
                    (self.pause)(MAX_BACKOFF);
                }
            }
        }
        false
    }
    pub(super) fn adopt(&mut self, reply: &Value, live: bool) -> SnapshotProgress {
        let reply: AttachMetadata<'_> = views::read(reply);
        if let Some(cursor) = reply.events.cursor {
            self.durable = self.durable.max(cursor);
        }
        if live {
            self.live = reply.live;
        }
        SnapshotProgress {
            events_empty: reply.events.events.is_empty(),
            approval: reply.agent.approval_id.map(str::to_owned),
            children: reply.children,
        }
    }
    /// Fetch a newly pending approval and newly spawned children.
    pub(super) fn after_reply(&mut self, progress: SnapshotProgress) -> bool {
        if progress.approval != self.approval {
            let detail = match &progress.approval {
                Some(id) => {
                    match self.read(&observe::value(Observation::Approval { approval_id: id })) {
                        Outcome::Ok(reply) => Some(views::read::<Detail>(&reply).approval),
                        // Keep the previous card; the next reply fetches it again.
                        failure => {
                            if !self.send(problem(failure)) {
                                return false;
                            }
                            None
                        }
                    }
                }
                None => Some(Value::Null),
            };
            if let Some(detail) = detail {
                self.approval = progress.approval;
                if !self.send(Update::Approval(detail)) {
                    return false;
                }
            }
        }
        if progress.children == 0 {
            self.children = Some(0);
            return true;
        }
        let announce = self.children.is_some();
        // Counts can stay equal when an old child is purged and a new one
        // appears. Always probe past the cursor, keeping no unbounded ID set.
        loop {
            let request = observe::value(Observation::Agents {
                parent: &self.target,
                limit: HISTORY_ROWS,
                after: self.child_after.unwrap_or(0),
            });
            let Outcome::Ok(reply) = self.read(&request) else {
                return !self.cancelled.load(Ordering::Acquire);
            };
            let reply: Listing = views::read(&reply);
            let rows = reply.agents;
            if rows.is_empty() {
                self.children = Some(progress.children);
                return true;
            }
            let Some(next) = reply
                .next_after
                .filter(|next| *next > self.child_after.unwrap_or(0))
            else {
                return self.send(Update::Problem(
                    String::from("child listing did not advance").into(),
                ));
            };
            if announce && !self.send(Update::Children(rows)) {
                return false;
            }
            self.child_after = Some(next);
        }
    }
}
