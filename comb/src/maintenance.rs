//! Native setup purge retains knowledge by default and revokes only its exact bridge.
mod bridge;

use crate::error::CommitExt;
use crate::{Comb, Error, Result, disk::Disk, provision, service};
use hive_setup_support::{
    files,
    maintenance::{Args, Plan, Scope},
    removal::{self, Action, Object},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
struct Details {
    detach: bool,
    remove_root: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    root_id: Option<crate::values::CombId>,
    #[serde(deserialize_with = "hive_input::decode::optional_object")]
    bridge: Option<bridge::Proof>,
    phase: Phase,
}
// The generated self-remote codec owns the field list; entry remains map-only.
impl Serialize for Details {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        Self::serialize(self, serializer)
    }
}
impl<'de> Deserialize<'de> for Details {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        struct Named;
        impl<'de> serde::de::Visitor<'de> for Named {
            type Value = Details;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("named maintenance details")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                Details::deserialize(serde::de::value::MapAccessDeserializer::new(map))
            }
        }
        decoder.deserialize_map(Named)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Admitted,
    Detached,
}
fn invalid(message: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::maintenance_configuration(message)
}
pub fn run(root: &Path, args: &Args) -> Result<serde_json::Value> {
    let plan = if let Some(receipt) = &args.apply {
        apply(receipt)?
    } else {
        prepare(root, args)?
    };
    crate::error::value(plan)
}
fn prepare(root: &Path, args: &Args) -> Result<Plan<Details>> {
    let root = removal::safe_path(args.knowledge_root.as_deref().unwrap_or(root))?;
    if args
        .purge_comb_root
        .as_deref()
        .map(removal::safe_path)
        .transpose()?
        .is_some_and(|p| p != root)
        || (args.purge_comb_root.is_some() && !args.detach_comb)
    {
        return Err(invalid(
            "--purge-comb-root must equal the selected root and requires removing Comb.",
        ));
    }
    let root_id = if root.join(".comb").exists() {
        Some(Disk::human_authority(&root)?.0)
    } else {
        None
    };
    if root.exists() && root_id.is_none() {
        return Err(invalid("Refusing an unrecognized knowledge root."));
    }
    let bridge_file = args
        .bridge_file
        .as_deref()
        .map(removal::safe_path)
        .transpose()?;
    if let Some(path) = &bridge_file {
        removal::disjoint(&[root.as_path(), path])?;
    }
    let proof = bridge_file
        .as_deref()
        .map(|p| bridge::inspect(&root, p))
        .transpose()?
        .flatten();
    let mut removals = vec![];
    if let Some(path) = &bridge_file {
        removals.push(Action::capture(path, "bridge")?);
    }
    let retained = vec![Object::capture(&root)?];
    Ok(Plan {
        format: hive_input::compatibility::Exact,
        product: "Comb".into(),
        scope: Scope {
            root: Some(root),
            bridge: bridge_file,
            ..Scope::default()
        },
        retained,
        removals,
        details: Details {
            detach: args.detach_comb,
            remove_root: args.purge_comb_root.is_some(),
            root_id,
            bridge: proof,
            phase: Phase::Prepared,
        },
    })
}
fn apply(receipt: &Path) -> Result<Plan<Details>> {
    publication_scope(|changed| apply_tracking(receipt, changed))
}
fn publication_scope<T>(operation: impl FnOnce(&mut bool) -> Result<T>) -> Result<T> {
    let mut changed = false;
    let result = operation(&mut changed);
    if changed { result.committed() } else { result }
}
fn apply_tracking(receipt: &Path, changed: &mut bool) -> Result<Plan<Details>> {
    let mut plan = Plan::<Details>::read(receipt, "Comb")?;
    let root = plan
        .scope
        .root
        .clone()
        .ok_or_else(|| invalid("Missing knowledge scope."))?;
    validate_receipt(&plan, &root)?;
    if !plan.details.detach {
        plan.check_retained()?;
        bridge::revoke(&mut plan, receipt, None, changed)?;
        return Ok(plan);
    }
    // Completed root quarantine retries cannot reopen the removed root.
    if plan.details.phase == Phase::Detached {
        return finish(plan, receipt, &root, changed);
    }
    plan.check_retained()?;
    let _absent_root = if plan.details.root_id.is_none() {
        Some(hive_setup_support::locks::path(&root, "comb-root", false)?)
    } else {
        None
    };
    plan.check_retained()?;
    let mut comb = if plan.details.root_id.is_some() {
        Some(Comb::open(&root)?)
    } else {
        None
    };
    check_detach_admission(&plan, &root, comb.as_mut(), changed)?;
    bridge::revoke(&mut plan, receipt, comb.as_mut(), changed)?;
    plan.check_retained()?;
    if plan.details.phase == Phase::Prepared {
        admit(&mut plan, &root)?;
        plan.save_tracking(receipt, changed)?;
    }
    plan.check_retained()?;
    detach_wiring(&root, changed)?;
    plan.details.phase = Phase::Detached;
    plan.save_tracking(receipt, changed)?;
    let _root = if let Some(mut comb) = comb {
        let guard = comb.disk.take_scope_lock()?;
        drop(comb);
        Some(guard)
    } else {
        None
    };
    remove_actions(&mut plan, receipt, changed)?;
    Ok(plan)
}
fn admit(plan: &mut Plan<Details>, root: &Path) -> Result<()> {
    if plan.details.remove_root {
        let mut action = Action::capture(root, "knowledge")?;
        if plan.retained[0].identity.is_none() {
            action.object.identity = None;
        } else {
            removal::tree(root)?;
        }
        plan.removals.push(action);
    }
    plan.details.phase = Phase::Admitted;
    Ok(())
}
fn finish(
    mut plan: Plan<Details>,
    receipt: &Path,
    root: &Path,
    changed: &mut bool,
) -> Result<Plan<Details>> {
    let _root = hive_setup_support::locks::path(root, "comb-root", false)?;
    if !plan.details.remove_root {
        plan.retained[0].check()?;
    }
    check_detached(root)?;
    remove_actions(&mut plan, receipt, changed)?;
    Ok(plan)
}
/// Purge admission detached the root's serving wiring; a resumed setup or
/// selection since then means the root is in use again.
fn check_detached(root: &Path) -> Result<()> {
    if root.join(".comb/serve.json").try_exists()?
        || root.join(".comb/model-transition.json").try_exists()?
        || root.join(".comb-setup.json").try_exists()?
    {
        return Err(invalid(
            "Knowledge setup was resumed after purge admission; refusing its cleanup.",
        ));
    }
    Ok(())
}
fn remove_actions(plan: &mut Plan<Details>, receipt: &Path, changed: &mut bool) -> Result<()> {
    for i in 0..plan.removals.len() {
        plan.remove_tracking(i, receipt, changed)?;
    }
    Ok(())
}

fn detach_wiring(root: &Path, changed: &mut bool) -> Result<()> {
    detach_with(root, changed, files::sync_parent)
}
fn detach_with(
    root: &Path,
    changed: &mut bool,
    sync: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    for name in ["serve.json", "model-transition.json"] {
        let path = root.join(".comb").join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => *changed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    // Repeat the barrier even if an earlier attempt already unlinked both files.
    match std::fs::metadata(root.join(".comb")) {
        Ok(_) => sync(&root.join(".comb/serve.json"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
#[cfg(test)]
mod tests;

fn validate_receipt(plan: &Plan<Details>, root: &Path) -> Result<()> {
    if plan.retained.iter().map(|o| o.path.as_path()).ne([root])
        || plan.scope.config.is_some()
        || plan.scope.state.is_some()
    {
        return Err(invalid("Invalid retained Comb scope."));
    }
    let mut expected = std::collections::BTreeSet::new();
    if let Some(path) = &plan.scope.bridge {
        removal::disjoint(&[root, path])?;
        expected.insert(path.clone());
    }
    if plan.details.phase != Phase::Prepared && plan.details.remove_root {
        expected.insert(root.to_owned());
    }
    let mut observed = std::collections::BTreeSet::new();
    for action in &plan.removals {
        if !expected.contains(&action.object.path) || !observed.insert(&action.object.path) {
            return Err(invalid("Purge action is outside native Comb ownership."));
        }
    }
    Ok(())
}

fn check_detach_admission(
    plan: &Plan<Details>,
    root: &Path,
    comb: Option<&mut Comb>,
    changed: &mut bool,
) -> Result<()> {
    if root.join(".comb-setup.json").try_exists()? {
        return Err(invalid(format!(
            "Finish fresh-root setup with `comb --root '{}' setup` before purge.",
            root.display()
        )));
    }
    if let Some(comb) = comb {
        if Some(comb.identity()) != plan.details.root_id {
            return Err(invalid("Knowledge root identity changed."));
        }
        let accepted = provision::pending_settings(root)?.is_some();
        provision::recover_locked(root, comb).map_err(|e| {
            let committed = e.file_committed;
            let mut mapped = invalid(format!(
                "Finish accepted activation with `comb --root '{}' serve` (and its required \
                    environment) before purge",
                root.display()
            ))
            .caused_by(e);
            mapped.file_committed = committed;
            mapped
        })?;
        *changed |= accepted;
    }
    Ok(())
}
