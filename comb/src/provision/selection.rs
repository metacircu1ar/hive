//! Root-scoped activation of the serving embedding selection. The root's
//! transition journal fences an accepted change until the index and the
//! server settings agree, including after a crash. The root writer lock
//! serializes every change and recovery of one root.
use super::{Path, PathBuf, Result, files, invalid};
use crate::{
    Comb,
    embedding::{Config, Embedder, Engine, HttpConfig},
    error::CommitExt,
    service,
};
use serde::{Deserialize, Serialize};
use std::fs;

const SETTINGS: &str = ".comb/serve.json";
const TRANSITION: &str = ".comb/model-transition.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServeSettings {
    pub root: PathBuf,
    pub socket: PathBuf,
    #[serde(deserialize_with = "hive_input::decode::optional_object")]
    pub embeddings: Option<Config>,
    /// Comb starts and owns this llama-server for the HTTP selection above.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "hive_input::decode::optional_object"
    )]
    pub llama_server: Option<crate::llama::ManagedServer>,
}
impl ServeSettings {
    pub(super) fn validate(&self) -> Result<()> {
        if !self.root.is_absolute() || !self.socket.is_absolute() {
            return Err(invalid("Invalid managed Comb server settings."));
        }
        if let Some(server) = &self.llama_server {
            server.validate(self.http())?;
        }
        Ok(())
    }
    /// The HTTP embedding configuration, when one is selected.
    pub fn http(&self) -> Option<&HttpConfig> {
        match &self.embeddings {
            Some(Config::Http(config)) => Some(config),
            None => None,
        }
    }
    /// The managed llama-server's port, when this selection has one.
    pub fn llama_port(&self) -> Option<u16> {
        crate::llama::previous_port(self.llama_server.as_ref(), self.http())
    }
}
#[derive(Serialize)]
pub struct Selected {
    pub changed: bool,
    pub settings: ServeSettings,
    pub backfill: &'static str,
}

/// The root's published server settings.
pub fn settings(root: &Path) -> Result<Option<ServeSettings>> {
    let root = root.canonicalize()?;
    read(&root, SETTINGS, "serve.json")
}
/// An accepted change not yet applied, without opening or recovering the root.
pub(crate) fn pending_settings(root: &Path) -> Result<Option<ServeSettings>> {
    let root = root.canonicalize()?;
    read(&root, TRANSITION, "model-transition.json")
}
/// Settings for a root whose writer lock is already held: an accepted change,
/// or else the published settings.
pub(super) fn settings_under_guard(disk: &crate::disk::Disk) -> Result<Option<ServeSettings>> {
    match read(&disk.root, TRANSITION, "model-transition.json")? {
        Some(pending) => Ok(Some(pending)),
        None => read(&disk.root, SETTINGS, "serve.json"),
    }
}
fn read(root: &Path, file: &str, name: &str) -> Result<Option<ServeSettings>> {
    let path = root.join(file);
    if !path.try_exists()? {
        return Ok(None);
    }
    let settings: ServeSettings = files::json(&path).map_err(|error| {
        if error.code == crate::ErrorCode::InvalidJson {
            invalid(format!(
                "Malformed managed {name}; inspect the root's server settings before restarting."
            ))
            .caused_by(error)
        } else {
            error
        }
    })?;
    settings.validate()?;
    if settings.root != root {
        return Err(invalid("Managed server settings belong to another root."));
    }
    Ok(Some(settings))
}

/// Called before opening a configured server. Never leaves permanent replacement enabled.
pub fn recover(root: &Path) -> Result<()> {
    recover_checked(root, &|_| Ok(()))
}
pub(super) fn recover_checked(
    root: &Path,
    check: &impl Fn(&crate::disk::Disk) -> Result<()>,
) -> Result<()> {
    let root = root.canonicalize()?;
    if root.join(TRANSITION).try_exists()? {
        let mut comb = Comb::open_checked(&root, check)?;
        recover_locked(&root, &mut comb)?;
    }
    Ok(())
}

/// Text search only.
pub fn select_none(root: &Path) -> Result<Selected> {
    change(root, None, None)
}
pub fn use_http(root: &Path, config: HttpConfig) -> Result<Selected> {
    change(root, Some(Config::Http(config)), None)
}
/// The managed llama-server selection. Its server must be ready: the change
/// probes the endpoint like any HTTP selection.
pub fn use_llama(
    root: &Path,
    config: HttpConfig,
    server: crate::llama::ManagedServer,
) -> Result<Selected> {
    change_selection(
        root,
        Some(Config::Http(config)),
        Some(server),
        None,
        &mut |_| Ok(()),
    )
}
pub(crate) fn select_none_for_setup(root: &Path, socket: &Path) -> Result<Selected> {
    change(root, None, Some(socket))
}
pub(crate) fn select_http_for_setup(
    root: &Path,
    http: HttpConfig,
    server: Option<crate::llama::ManagedServer>,
    socket: &Path,
) -> Result<Selected> {
    change_selection(
        root,
        Some(Config::Http(http)),
        server,
        Some(socket),
        &mut |_| Ok(()),
    )
}
/// Recover an accepted selection, then compare it with the index under one guard.
pub(crate) fn reconcile_for_setup(root: &Path) -> Result<Selected> {
    let root = root.canonicalize()?;
    let mut comb = Comb::open(&root)?;
    let accepted = pending_settings(&root)?.is_some();
    let result = (|| {
        recover_locked(&root, &mut comb)?;
        let saved = settings(&root)?.ok_or_else(|| {
            invalid("No managed settings; select text search or an HTTP embedding configuration.")
        })?;
        let serving = serving_space(&saved)?
            .as_ref()
            .map(crate::embedding::Space::fingerprint)
            .transpose()?;
        let active = comb
            .index()?
            .active_space()?
            .map(|(lease, _)| lease.fingerprint);
        if serving != active {
            return Err(crate::Error::embedding_replace_required());
        }
        Ok(Selected {
            changed: false,
            settings: saved,
            backfill: "unchanged",
        })
    })();
    if accepted { result.committed() } else { result }
}
/// The space a selection serves.
pub fn serving_space(settings: &ServeSettings) -> Result<Option<crate::embedding::Space>> {
    match &settings.embeddings {
        Some(Config::Http(config)) => Ok(Some(Engine::http_identity(config.clone())?)),
        None => Ok(None),
    }
}
fn change(root: &Path, embeddings: Option<Config>, socket: Option<&Path>) -> Result<Selected> {
    change_with_hook(root, embeddings, socket, &mut |_| Ok(()))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    BeforeJournal,
    JournalRenamed,
    JournalPublished,
    IndexActivated,
    ConfigPublished,
}
pub(super) fn change_with_hook(
    root: &Path,
    embeddings: Option<Config>,
    socket: Option<&Path>,
    hook: &mut impl FnMut(Phase) -> Result<()>,
) -> Result<Selected> {
    change_selection(root, embeddings, None, socket, hook)
}
fn change_selection(
    root: &Path,
    embeddings: Option<Config>,
    llama_server: Option<crate::llama::ManagedServer>,
    socket: Option<&Path>,
    hook: &mut impl FnMut(Phase) -> Result<()>,
) -> Result<Selected> {
    let root = root.canonicalize()?;
    // The root writer lock also rejects an unmanaged live server, before any change.
    let mut comb = Comb::open(&root)?;
    recover_locked(&root, &mut comb)?;
    let old = settings(&root)?;
    if socket.is_some_and(|path| old.as_ref().is_some_and(|s| s.socket != path)) {
        return Err(invalid(
            "Root socket differs from requested setup; inspect serve.json.",
        ));
    }
    let settings = ServeSettings {
        socket: old.as_ref().map_or_else(
            || service::socket_path(&root, socket),
            |s| Ok(s.socket.clone()),
        )?,
        root: root.clone(),
        embeddings,
        llama_server,
    };
    settings.validate()?;
    let space = serving_space(&settings)?;
    if old.as_ref().map(serde_json::to_vec).transpose()? == Some(serde_json::to_vec(&settings)?)
        && comb
            .index()?
            .active_space()?
            .map(|(lease, _)| lease.fingerprint)
            == space
                .as_ref()
                .map(crate::embedding::Space::fingerprint)
                .transpose()?
    {
        return Ok(Selected {
            changed: false,
            settings,
            backfill: "unchanged",
        });
    }
    let mut engine = validated_engine(&settings, true)?;
    commit_selection(settings, &mut comb, engine.as_mut(), hook)
}
fn commit_selection(
    settings: ServeSettings,
    comb: &mut Comb,
    engine: Option<&mut Engine>,
    hook: &mut impl FnMut(Phase) -> Result<()>,
) -> Result<Selected> {
    hook(Phase::BeforeJournal)?;
    files::write_json_hook(&settings.root.join(TRANSITION), &settings, &mut |phase| {
        if phase == files::Publish::Renamed {
            hook(Phase::JournalRenamed)
        } else {
            Ok(())
        }
    })
    .map_err(|error| {
        if error.file_committed {
            pending(error)
        } else {
            error
        }
    })?;
    hook(Phase::JournalPublished).map_err(pending)?;
    finish(&settings, comb, engine, hook).map_err(pending)?;
    let backfill = if settings.embeddings.is_some() {
        "pending; serve starts background indexing, text search stays available"
    } else {
        "disabled; text search only"
    };
    Ok(Selected {
        changed: true,
        settings,
        backfill,
    })
}
fn validated_engine(settings: &ServeSettings, probe: bool) -> Result<Option<Engine>> {
    let expected = serving_space(settings)?;
    let Some(config) = &settings.embeddings else {
        return Ok(None);
    };
    let mut engine = Engine::load(config.clone())?;
    if Some(engine.space()) != expected.as_ref() {
        return Err(invalid(
            "Embedding identity differs from its configuration.",
        ));
    }
    if probe {
        probe_engine(&mut engine)?;
    }
    Ok(Some(engine))
}
pub(super) fn probe_engine(engine: &mut Engine) -> Result<()> {
    let inputs = [
        format!("{}x", engine.space().query_prefix),
        format!("{}x", engine.space().document_prefix),
    ];
    let vectors = engine.embed(&inputs)?;
    if vectors.len() != inputs.len() {
        return Err(invalid("Embedding probe returned the wrong vector count."));
    }
    for vector in vectors {
        crate::embedding::normalize(vector?, engine.space().dimensions())?;
    }
    Ok(())
}
/// Finish an accepted change. Its caller holds the root writer lock.
pub(crate) fn recover_locked(root: &Path, comb: &mut Comb) -> Result<()> {
    let Some(settings) = read(root, TRANSITION, "model-transition.json")? else {
        return Ok(());
    };
    let mut engine = validated_engine(&settings, false).map_err(pending)?;
    finish(&settings, comb, engine.as_mut(), &mut |_| Ok(())).map_err(pending)
}
fn finish(
    settings: &ServeSettings,
    comb: &mut Comb,
    engine: Option<&mut Engine>,
    hook: &mut impl FnMut(Phase) -> Result<()>,
) -> Result<()> {
    if let Some(engine) = engine {
        comb.activate_engine(engine, true)?;
    } else {
        comb.deactivate_embeddings()?;
    }
    hook(Phase::IndexActivated)?;
    // If any following write fails, the durable journal remains. Recovery repeats
    // activation idempotently; it never claims the old index was restored.
    files::write_json(&settings.root.join(SETTINGS), settings)?;
    hook(Phase::ConfigPublished)?;
    clear_transition(&settings.root)
}
fn clear_transition(root: &Path) -> Result<()> {
    match fs::remove_file(root.join(TRANSITION)) {
        Ok(()) => fs::File::open(root.join(".comb"))?.sync_all()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn pending(error: crate::Error) -> crate::Error {
    crate::Error::model_activation_pending()
        .committed()
        .caused_by(error)
}
