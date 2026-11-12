//! Durable previous-loaded set, retained scope locks and recovery by actual pointer.
use crate::{
    Result,
    lifecycle::{Change, Lifecycle, Operation},
    publish,
};
use hive_setup_support::{
    files,
    runner::{Runner, SystemRunner},
    service::{Controller, Held, Layout, Recovery, Spec},
};
use serde::Deserialize;
use std::{fs, path::Path, time::Duration};
const PENDING: &str = "pending-install.json";
mod links;
mod recovery;
mod restoration;
use restoration::{present, present_mut};
mod record;
use record::{Journal, Saved};
#[derive(Deserialize)]
struct Health {
    ready: bool,
    loaded_agents: Option<usize>,
    active_turns: Option<usize>,
}
type Probe = dyn Fn(&Spec) -> Result<Health>;
pub struct Services {
    yes: bool,
    runner: Box<dyn Runner>,
    health: Box<Probe>,
    layout: Layout,
    held: Vec<Recovery>,
    journal: Option<Journal>,
}
impl Services {
    pub(crate) fn removal_specs(
        &self,
        prefix: &Path,
        components: &[String],
    ) -> Result<(Layout, Vec<Spec>)> {
        Ok((
            self.layout.clone(),
            inventory(&self.layout, prefix, components)?,
        ))
    }
    pub fn new(yes: bool) -> Result<Self> {
        Ok(Self {
            yes,
            runner: Box::new(SystemRunner(std::process::Command::spawn)),
            health: Box::new(probe),
            layout: Layout::discover()?,
            held: vec![],
            journal: None,
        })
    }
    fn acquire(&self, specs: &[Spec]) -> Result<Vec<Recovery>> {
        let runner = self.runner.as_ref();
        let mut sorted = specs.to_vec();
        sorted.sort_by_key(|s| s.descriptor.clone());
        let mut held = vec![];
        for spec in sorted {
            held.push(Recovery::Present(Box::new(
                Controller {
                    layout: &self.layout,
                    scope: &spec.scope,
                    runner,
                }
                .hold_expected(&spec)?,
            )));
        }
        Ok(held)
    }
    fn clear_journal(&mut self) -> Result<()> {
        let journal = self.journal.as_ref().ok_or("Missing service transaction")?;
        files::remove(
            &journal.change.manager.join(PENDING),
            &files::encode(journal)?,
        )?;
        self.journal = None;
        self.held.clear();
        Ok(())
    }
}
impl Lifecycle for Services {
    fn preview_removal(&self, prefix: &Path, components: &[String]) -> Result<()> {
        for spec in inventory(&self.layout, prefix, components)? {
            eprintln!(
                "Remove owned service {}: {} and {}",
                spec.scope.label()?,
                spec.unit.display(),
                spec.descriptor.display()
            );
        }
        Ok(())
    }
    fn recover(&mut self, prefix: &Path, manager: &Path) -> Result<()> {
        self.recover_pending(prefix, manager)
    }
    fn prepare(&mut self, change: &Change) -> Result<()> {
        let specs = inventory(&self.layout, &change.prefix, &change.components)?;
        self.held = self.acquire(&specs)?;
        let runner = self.runner.as_ref();
        let mut saved = vec![];
        for ownership in &self.held {
            let held = present(ownership)?;
            let loaded = held.loaded(runner)?;
            let health = (self.health)(held.spec())?;
            if health.ready && !loaded {
                return Err(
                    "A manual instance is using an owned service scope; stop it before upgrading"
                        .into(),
                );
            }
            eprintln!(
                "Service {}: {}, {} loaded agents, {} active turns.",
                held.spec().scope.label()?,
                if change.operation == Operation::Uninstall {
                    "will be uninstalled"
                } else if loaded {
                    "running/starting; will be restarted"
                } else {
                    "stopped; will stay stopped"
                },
                health.loaded_agents.map_or("n/a".into(), |n| n.to_string()),
                health.active_turns.map_or("n/a".into(), |n| n.to_string())
            );
            saved.push(Saved {
                spec: held.spec().clone(),
                next: held.spec().rendered()?,
                loaded,
            });
        }
        if change.operation == Operation::Uninstall || saved.iter().any(|s| s.loaded) {
            confirm(
                self.yes,
                "Stop the displayed owned services for this installation change?",
            )?;
        }
        let journal = Journal {
            format: hive_input::compatibility::Exact,
            change: change.clone(),
            services: saved,
        };
        files::write(
            &change.manager.join(PENDING),
            &files::encode(&journal)?,
            false,
        )?;
        self.journal = Some(journal);
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        let runner = self.runner.as_ref();
        for ownership in &self.held {
            let held = present(ownership)?;
            held.stop(runner, &|| {
                (self.health)(held.spec())
                    .map(|p| p.ready)
                    .map_err(crate::Error::into_io_other)
            })?;
        }
        let journal = self.journal.as_ref().ok_or("Missing transaction")?;
        for (ownership, saved) in self.held.iter_mut().zip(&journal.services) {
            let held = present_mut(ownership)?;
            if journal.change.operation == Operation::Uninstall {
                held.uninstall(runner, &|| {
                    (self.health)(&saved.spec)
                        .map(|p| p.ready)
                        .map_err(crate::Error::into_io_other)
                })?;
            } else {
                held.restore_unit(runner, &saved.spec, &saved.next, true)?;
            }
        }
        Ok(())
    }
    fn restore(&mut self, published: bool) -> Result<()> {
        let journal = self.journal.as_ref().ok_or("Missing service transaction")?;
        let expected = if published {
            Some(journal.change.next.clone())
        } else {
            journal.change.previous.clone()
        };
        if publish::current(&journal.change.manager)? != expected {
            return Err("Cannot confirm publication before restoring services".into());
        }
        self.restore_all(published)
    }
    fn clear(&mut self) -> Result<()> {
        self.clear_journal()
    }
}
fn probe(spec: &Spec) -> Result<Health> {
    let mut args = match (spec.scope.product.as_str(), spec.scope.paths.as_slice()) {
        ("hive", [config, state]) => vec![
            "--config".into(),
            config.to_string_lossy().into_owned(),
            "--state-dir".into(),
            state.to_string_lossy().into_owned(),
        ],
        ("comb", [root]) => vec!["--root".into(), root.to_string_lossy().into_owned()],
        _ => return Err("Invalid service scope".into()),
    };
    args.extend(["service".into(), "status".into()]);
    let binary = if spec.scope.product == "hive" {
        spec.program
            .parent()
            .ok_or("Service executable has no parent")?
            .join("hivectl")
    } else {
        spec.program.clone()
    };
    let output = crate::native::capture(
        binary.to_str().ok_or("Non-UTF8 service program")?,
        &args,
        PROBE_TIMEOUT,
    )?;
    if output.code != 0 {
        return Err("Cannot establish owned service health; native command failed.".into());
    }
    Ok(serde_json::from_str(&output.stdout)?)
}
fn inventory(layout: &Layout, prefix: &Path, components: &[String]) -> Result<Vec<Spec>> {
    let directory = layout.config.join("hive/services");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut specs = vec![];
    for (i, entry) in entries.enumerate() {
        if i > 4096 {
            return Err("Too many service records to inspect safely".into());
        }
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json")
            || path.file_name().is_some_and(|n| {
                n.to_string_lossy().ends_with(".failure.json")
                    || n.to_string_lossy().ends_with(".refresh.json")
            })
        {
            continue;
        }
        let Some(spec): Option<Spec> =
            files::versioned_json(&path, &hive_setup_support::compatibility::SERVICE)?
        else {
            continue;
        };
        if path != layout.descriptor(&spec.scope)? {
            return Err("Service record is stored under the wrong label".into());
        }
        if !components.contains(&spec.scope.product) {
            continue;
        }
        if !owned_program(&spec, prefix) {
            continue;
        }
        spec.verify(layout, &spec.scope)?;
        specs.push(spec);
    }
    specs.sort_by_key(|s| s.descriptor.clone());
    if specs.len() > 256 {
        return Err("Too many affected services for one installation transaction".into());
    }
    Ok(specs)
}
fn owned_program(spec: &Spec, prefix: &Path) -> bool {
    match spec.scope.product.as_str() {
        "hive" => ["hivectl", "hive-daemon"]
            .iter()
            .any(|name| spec.program == prefix.join("bin").join(name)),
        "comb" => spec.program == prefix.join("bin/comb"),
        _ => false,
    }
}
fn validate(journal: &Journal, prefix: &Path, manager: &Path) -> Result<()> {
    let change = &journal.change;
    let pointer = |p: &Path| {
        p.components().count() == 2
            && p.starts_with("generations")
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("g-"))
            && p.to_str().is_some_and(crate::manifest::safe_path)
    };
    if change.prefix != prefix
        || change.manager != manager
        || manager != prefix.join("lib/hive")
        || !pointer(&change.next)
        || change.previous.as_deref().is_some_and(|p| !pointer(p))
        || change.components.is_empty()
        || change.components.iter().any(|s| {
            !matches!(
                s.as_str(),
                "hive" | "comb" | crate::manifest::LLAMA_COMPONENT
            )
        })
        || journal.services.len() > 256
    {
        return Err("Invalid pending installation scope".into());
    }
    let old = publish::load(
        manager,
        change.previous.as_deref(),
        &crate::manifest::native_target()?,
    )?;
    let old_links = publish::links(&old.parts)?;
    links::validate(journal, &old_links)?;
    let mut retained = old.parts.clone();
    for component in &change.components {
        retained.remove(component);
    }
    let retained_links = publish::links(&retained)?;
    let expected_removed: std::collections::BTreeMap<_, _> =
        if change.operation == Operation::Uninstall {
            old_links
                .iter()
                .filter(|(name, _)| !retained_links.contains_key(*name))
                .map(|(name, target)| (name.clone(), target.clone()))
                .collect()
        } else {
            std::collections::BTreeMap::default()
        };
    if change.removed_links != expected_removed
        || (change.operation == Operation::Uninstall && !change.added_links.is_empty())
    {
        return Err("Invalid pending removal ownership".into());
    }
    for pair in journal.services.windows(2) {
        if pair[0].spec.descriptor >= pair[1].spec.descriptor {
            return Err("Unordered or duplicate service recovery scopes".into());
        }
    }
    if journal.services.iter().any(|s| {
        !owned_program(&s.spec, prefix) || !change.components.contains(&s.spec.scope.product)
    }) {
        return Err("Pending services are outside this installation".into());
    }
    Ok(())
}
pub(crate) fn confirm(yes: bool, question: &str) -> Result<()> {
    use std::io::{Read, Write};
    if yes {
        return Ok(());
    }
    let mut tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|cause| {
            crate::publication::context(
                "Confirmation needs a controlling terminal; review the displayed scope and \
                pass --yes",
                cause.into(),
            )
        })?;
    write!(tty, "{question} [y/N] (--yes): ")?;
    tty.flush()?;
    let mut input = vec![];
    for _ in 0..128 {
        let mut b = [0];
        if tty.read(&mut b)? == 0 || b[0] == b'\n' {
            break;
        }
        input.push(b[0]);
    }
    if !matches!(
        String::from_utf8(input)?
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "y" | "yes"
    ) {
        return Err("Installation cancelled before stopping services".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

pub(crate) fn reconcile_removed(
    prefix: &Path,
    removed: &std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    restore: bool,
) -> Result<()> {
    for (name, target) in removed {
        let path = prefix.join(name);
        match fs::read_link(&path) {
            Ok(current) if current == *target => {
                if !restore {
                    fs::remove_file(&path)?;
                }
            }
            Ok(_) => return Err("Removed command link was edited; refusing recovery".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if restore {
                    std::os::unix::fs::symlink(target, &path)?;
                }
            }
            Err(e) => return Err(e.into()),
        }
        files::sync_parent(&path)?;
    }
    Ok(())
}

const PROBE_TIMEOUT: std::time::Duration = Duration::from_secs(15);
