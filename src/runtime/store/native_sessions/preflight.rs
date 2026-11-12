//! Bound owned-file work and outbox size before an agent loses its ownership.
use super::{
    files,
    planning::{Binding, generation, invalid, provider},
    queries,
};
use crate::{error::Result, runtime::store::Store};
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};

type Session = (String, String, String);
const GC_WORK: usize = 100_000;
const GC_FILES: usize = 1_024;

pub(in crate::runtime::store) struct Planner {
    plans: BTreeMap<Session, files::Plan>,
    owned_budget: files::Budget,
    binding_budget: files::Budget,
    remaining_files: Option<usize>,
    reserved: BTreeSet<Session>,
}

impl Planner {
    pub fn purge() -> Self {
        Self::new(None, None)
    }
    pub fn gc() -> Self {
        Self::new(Some(GC_WORK), Some(GC_FILES))
    }
    pub(in crate::runtime::store::native_sessions) fn new(
        work: Option<usize>,
        files: Option<usize>,
    ) -> Self {
        Self {
            plans: BTreeMap::new(),
            owned_budget: files::Budget::new(work),
            binding_budget: files::Budget::new(work.map(|_| GC_WORK)),
            remaining_files: files,
            reserved: BTreeSet::new(),
        }
    }
    fn discover(&mut self, store: &Store, binding: &Binding) -> Result<Option<files::Plan>> {
        let key = (
            binding.generation.clone(),
            binding.provider.clone(),
            binding.thread.clone(),
        );
        if !self.plans.contains_key(&key) {
            if self.remaining_files.is_none() {
                self.plans.clear();
            }
            let mut locations = Vec::new();
            let mut after = String::new();
            loop {
                let rows: Vec<super::locations::Location> = store.sql().cached().query_rows(
                    queries::LOCATIONS,
                    params![binding.generation, binding.provider, binding.thread, after],
                )?;
                if rows.is_empty() {
                    break;
                }
                for row in rows {
                    if !self.owned_budget.charge() {
                        return Ok(None);
                    }
                    if locations.len() == GC_WORK {
                        return Ok(Some(files::Plan {
                            warning: Some(crate::runtime::api::NativeWarningReason::ScanLimit),
                            ..files::Plan::default()
                        }));
                    }
                    after.clone_from(&row.path);
                    locations.push(row);
                }
            }
            let Some(plan) = files::discover(
                &store.native_home(generation(&binding.generation)?)?,
                provider(&binding.provider)?,
                &binding.thread,
                &locations,
                &mut self.owned_budget,
            )?
            else {
                return Ok(None);
            };
            self.plans.insert(key.clone(), plan);
        }
        Ok(Some(self.plans.get(&key).ok_or_else(invalid)?.clone()))
    }
    pub(super) fn plan(&mut self, store: &Store, binding: &Binding) -> Result<files::Plan> {
        // GC has already admitted every binding; this is a cache lookup only.
        self.discover(store, binding)?.ok_or_else(invalid)
    }
    pub fn admit(&mut self, store: &Store, agent: &str) -> Result<bool> {
        let mut cursor = (String::new(), String::new(), String::new());
        let mut sessions = BTreeSet::new();
        let mut files = 0;
        loop {
            let bindings: Vec<Binding> = store.sql().cached().query_rows(
                queries::AGENT_SESSIONS,
                params![agent, cursor.0, cursor.1, cursor.2],
            )?;
            if bindings.is_empty() {
                break;
            }
            for binding in bindings {
                if !self.binding_budget.charge() {
                    return Ok(false);
                }
                let key = (
                    binding.generation.clone(),
                    binding.provider.clone(),
                    binding.thread.clone(),
                );
                let shared: bool = store.query_row_cached(
                    queries::ADMISSION_SURVIVING_OWNER,
                    params![binding.generation, binding.provider, binding.thread, agent],
                    |r| r.get(0),
                )?;
                if !shared && !self.reserved.contains(&key) {
                    let Some(plan) = self.discover(store, &binding)? else {
                        return Ok(false);
                    };
                    files += plan.files.len();
                    if self
                        .remaining_files
                        .is_some_and(|remaining| files > remaining)
                    {
                        return Ok(false);
                    }
                    sessions.insert(key);
                }
                cursor = (binding.generation, binding.provider, binding.thread);
            }
        }
        if let Some(remaining) = &mut self.remaining_files {
            *remaining -= files;
        }
        self.reserved.extend(sessions);
        Ok(true)
    }
}
