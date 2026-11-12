//! Foreground ownership exists before the native task-registration message.
mod admission;
mod pending;
mod routing;
#[cfg(test)]
mod tests;

use super::{Result, protocol, wire};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

pub(super) use admission::AGENT_REASON;
pub(super) use pending::{PendingControls, Registration};

pub(super) const TASK_LIMIT: usize = 64;
pub(super) const REGISTRATION_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    Parent,
    Child(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Agent,
    Skill,
}

impl Kind {
    fn of(name: &str) -> Option<Self> {
        match wire::NativeTool::parse(name) {
            wire::NativeTool::Agent => Some(Self::Agent),
            wire::NativeTool::Skill => Some(Self::Skill),
            _ => None,
        }
    }
}

struct Owner {
    key: u64,
    kind: Kind,
    task: Option<String>,
    observed_child: bool,
    closed: Option<Instant>,
}

struct Retired {
    kind: Kind,
    registered: bool,
    // A first child frame may itself overtake registration after owner closure.
    // Preserve that evidence without reopening any tool admission.
    pending: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Tasks {
    owners: HashMap<String, Owner>,
    // Preserve whether this owner already bound its one native task. A closed
    // inline/rejected call may reconcile one delayed first registration only.
    retired: HashMap<String, Retired>,
    retired_ids: HashSet<String>,
    seen_ids: HashSet<String>,
    // Keep execution evidence separately from held permission payloads: native
    // cancellation/denial does not prove that a child inference has ended.
    unresolved: HashMap<String, Option<Instant>>,
    next_key: u64,
}

impl Tasks {
    pub(super) fn identities(&self) -> usize {
        self.owners.len()
            + self.owners.values().filter(|o| o.task.is_some()).count()
            + self.retired.len()
            + self.retired_ids.len()
            + self.seen_ids.len()
            + self.unresolved.len()
    }
    pub(super) fn can_open(&self, name: &str) -> bool {
        let pending = self
            .retired
            .values()
            .filter(|owner| owner.pending.is_some());
        self.owners.len() + pending.clone().count() < TASK_LIMIT
            && Kind::of(name).is_some_and(|kind| {
                kind != Kind::Skill
                    || (!self.owners.values().any(|owner| owner.kind == Kind::Skill)
                        && !pending.clone().any(|owner| owner.kind == Kind::Skill))
            })
    }
    pub(super) fn open(&mut self, tool: &str, name: &str) -> Result<()> {
        let Some(kind) = Kind::of(name) else {
            return Ok(());
        };
        if !self.can_open(name) || self.owners.contains_key(tool) || self.retired.contains_key(tool)
        {
            return Err(protocol());
        }
        self.next_key = self.next_key.checked_add(1).ok_or_else(protocol)?;
        self.owners.insert(
            tool.into(),
            Owner {
                key: self.next_key,
                kind,
                task: None,
                observed_child: false,
                closed: None,
            },
        );
        Ok(())
    }
    pub(super) fn frame_scope(&mut self, parent: &str) -> Result<Option<Scope>> {
        if let Some(owner) = self.owners.get_mut(parent) {
            owner.observed_child = true;
            return Ok(owner.closed.is_none().then_some(Scope::Child(owner.key)));
        }
        if let Some(owner) = self.retired.get_mut(parent) {
            if !owner.registered {
                owner
                    .pending
                    .get_or_insert(Instant::now() + REGISTRATION_WAIT);
            }
            return Ok(None);
        }
        Err(protocol())
    }
    pub(super) fn child_scope(&self, agent: &str) -> Option<Scope> {
        self.owners
            .values()
            .find(|owner| owner.closed.is_none() && owner.task.as_deref() == Some(agent))
            .map(|owner| Scope::Child(owner.key))
    }
    pub(super) fn retired_child(&self, agent: &str) -> bool {
        self.retired_ids.contains(agent)
    }
    pub(super) fn unbound(&self) -> bool {
        self.owners
            .values()
            .any(|o| o.closed.is_none() && o.task.is_none())
    }
    pub(super) fn active(&self) -> bool {
        self.owners.values().any(|owner| owner.closed.is_none())
    }
    pub(super) fn proof_pending(&self) -> bool {
        !self.owners.is_empty()
            || !self.unresolved.is_empty()
            || self.retired.values().any(|owner| owner.pending.is_some())
    }
    pub(super) fn observe_unresolved(&mut self, agent: &str, now: Instant) {
        let deadline = (!self.unbound()).then_some(now + REGISTRATION_WAIT);
        self.unresolved.entry(agent.into()).or_insert(deadline);
    }
    pub(super) fn register(&mut self, fields: &wire::System) -> Result<()> {
        let tool = fields.tool_use_id.as_deref().ok_or_else(protocol)?;
        let task = fields
            .task_id
            .as_deref()
            .filter(|id| valid_id(id))
            .ok_or_else(protocol)?;
        if fields.task_type.as_deref() != Some("local_agent")
            || fields.spawn_depth != Some(1)
            || fields.is_backgrounded != Some(false)
            || self.seen_ids.contains(task)
        {
            return Err(protocol());
        }
        if let Some(owner) = self.owners.get_mut(tool) {
            if owner.task.is_some() {
                return Err(protocol());
            }
            owner.task = Some(task.into());
        } else if let Some(owner) = self.retired.get_mut(tool) {
            if owner.registered {
                return Err(protocol());
            }
            owner.registered = true;
            owner.pending = None;
            // Metadata can lag a completed call. It never authorizes more work.
            self.retired_ids.insert(task.into());
        } else {
            return Err(protocol());
        }
        self.seen_ids.insert(task.into());
        self.unresolved.remove(task);
        self.retire_ready();
        Ok(())
    }
    pub(super) fn update(&self, kind: wire::SystemSubtype, fields: &wire::System) -> Result<()> {
        if fields.is_backgrounded == Some(true)
            || fields
                .patch
                .as_ref()
                .is_some_and(|patch| patch.is_backgrounded == Some(true))
        {
            return Err(protocol());
        }
        let task = fields.task_id.as_deref().ok_or_else(protocol)?;
        if self.retired_ids.contains(task) {
            return Ok(());
        }
        let (tool, _) = self
            .owners
            .iter()
            .find(|(_, owner)| owner.task.as_deref() == Some(task))
            .ok_or_else(protocol)?;
        if (kind == wire::SystemSubtype::TaskNotification
            && fields.tool_use_id.as_deref() != Some(tool))
            || fields.tool_use_id.as_deref().is_some_and(|id| id != tool)
        {
            return Err(protocol());
        }
        Ok(())
    }
    pub(super) fn close(&mut self, tool: &str, now: Instant) -> Option<Scope> {
        let owner = self.owners.get_mut(tool)?;
        owner.closed.get_or_insert(now + REGISTRATION_WAIT);
        let scope = Scope::Child(owner.key);
        self.retire_ready();
        if !self.unbound() {
            for deadline in self.unresolved.values_mut() {
                deadline.get_or_insert(now + REGISTRATION_WAIT);
            }
        }
        Some(scope)
    }
    pub(super) fn open_owners(&self) -> Vec<String> {
        self.owners
            .iter()
            .filter(|(_, o)| o.closed.is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }
    fn retire_ready(&mut self) {
        // An unrelated unresolved ID cannot be assigned to an owner by order.
        // Keep closed slots conservatively until exact registration reconciles it.
        let unresolved = !self.unresolved.is_empty();
        let retired = self.owners.extract_if(|_, owner| {
            owner.closed.is_some()
                && (owner.task.is_some() || (!owner.observed_child && !unresolved))
        });
        for (tool, owner) in retired {
            let registered = owner.task.is_some();
            if let Some(task) = owner.task {
                self.retired_ids.insert(task);
            }
            self.retired.insert(
                tool,
                Retired {
                    kind: owner.kind,
                    registered,
                    pending: None,
                },
            );
        }
    }
    pub(super) fn expired(&self, now: Instant) -> bool {
        self.owners
            .values()
            .any(|owner| owner.closed.is_some_and(|deadline| now >= deadline))
            || self
                .unresolved
                .values()
                .any(|d| d.is_some_and(|deadline| now >= deadline))
            || self
                .retired
                .values()
                .any(|owner| owner.pending.is_some_and(|deadline| now >= deadline))
    }
    pub(super) fn wait(&self, timeout: Duration) -> Duration {
        self.owners
            .values()
            .filter_map(|owner| owner.closed)
            .chain(self.unresolved.values().filter_map(|deadline| *deadline))
            .chain(self.retired.values().filter_map(|owner| owner.pending))
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .fold(timeout, Duration::min)
    }
}

pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= super::limits::TOOL_ID_BYTES
}
