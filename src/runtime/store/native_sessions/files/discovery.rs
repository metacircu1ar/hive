//! Inspect recorded transcripts and exact session-owned directories only.
use super::super::locations::Location;
use super::{
    Error, File, Identity, Path, PathBuf, Plan, Provider, Result, Warning, canonical_session,
    checked_parent, fs, home_metadata, metadata, transcript_path,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 12;

pub(in crate::runtime::store::native_sessions) struct Budget {
    remaining: Option<usize>,
    pub exhausted: bool,
}
impl Budget {
    pub fn new(remaining: Option<usize>) -> Self {
        Self {
            remaining,
            exhausted: false,
        }
    }
    pub fn charge(&mut self) -> bool {
        if let Some(remaining) = &mut self.remaining {
            if *remaining == 0 {
                self.exhausted = true;
                return false;
            }
            *remaining -= 1;
        }
        true
    }
}

pub(in crate::runtime::store::native_sessions) fn discover(
    home: &Path,
    provider: Provider,
    session: &str,
    locations: &[Location],
    budget: &mut Budget,
) -> Result<Option<Plan>> {
    let mut scan = Scan {
        home,
        budget,
        visited: 0,
        files: BTreeMap::new(),
        warning: None,
    };
    let result = if !canonical_session(session) {
        scan.warning = Some(Warning::InvalidSessionIdentity);
        Ok(())
    } else if locations.is_empty() {
        scan.warning = Some(Warning::IncompleteHistory);
        Ok(())
    } else {
        scan.scan(provider, session, locations)
    };
    match result {
        Err(Error::UnsafeState { .. }) => scan.warning = Some(Warning::UnsafePath),
        other => other?,
    }
    if scan.budget.exhausted {
        return Ok(None);
    }
    Ok(Some(Plan {
        files: if scan.warning.is_some() {
            Vec::new()
        } else {
            scan.files.into_values().collect()
        },
        warning: scan.warning,
    }))
}

struct Scan<'a> {
    home: &'a Path,
    budget: &'a mut Budget,
    visited: usize,
    files: BTreeMap<PathBuf, File>,
    warning: Option<Warning>,
}
impl Scan<'_> {
    fn scan(&mut self, provider: Provider, session: &str, locations: &[Location]) -> Result<()> {
        if home_metadata(self.home)?.is_none() {
            return Ok(());
        }
        let mut transcripts = BTreeSet::new();
        let mut projects = BTreeSet::new();
        for location in locations {
            let path = Path::new(&location.path);
            if !transcript_path(provider, session, path) {
                return Err(super::invalid_path());
            }
            transcripts.insert(path.to_path_buf());
            if provider == Provider::Codex {
                let name = path.file_name().ok_or_else(super::invalid_path)?;
                transcripts.insert(Path::new("archived_sessions").join(name));
            } else {
                projects.insert(path.parent().ok_or_else(super::invalid_path)?.to_path_buf());
            }
        }
        for path in transcripts {
            self.file(&path)?;
        }
        if self.files.len() > 1 {
            self.warning = Some(Warning::AmbiguousSession);
            return Ok(());
        }
        if provider == Provider::Codex {
            return Ok(());
        }
        if self.files.is_empty() && !locations.iter().any(|location| location.verified) {
            self.warning = Some(Warning::IncompleteHistory);
            return Ok(());
        }
        for project in projects {
            self.walk(&project.join(session), 0)?;
        }
        for root in ["file-history", "session-env", "tasks"] {
            self.walk(&Path::new(root).join(session), 0)?;
        }
        self.file(&Path::new("debug").join(format!("{session}.txt")))?;
        self.todos(session)
    }
    fn visit(&mut self) -> bool {
        if self.warning.is_some() || !self.budget.charge() {
            return false;
        }
        self.visited += 1;
        if self.visited > MAX_ENTRIES {
            self.warning = Some(Warning::ScanLimit);
        }
        self.warning.is_none()
    }
    fn file(&mut self, relative: &Path) -> Result<()> {
        if !self.visit() {
            return Ok(());
        }
        if relative.to_str().is_none() {
            self.warning = Some(Warning::UnsafePath);
            return Ok(());
        }
        let (_, present) = checked_parent(self.home, relative)?;
        if present && let Some(meta) = metadata(&self.home.join(relative), false)? {
            self.files.insert(
                relative.into(),
                File {
                    path: relative.into(),
                    identity: Identity::of(&meta),
                },
            );
        }
        Ok(())
    }
    fn directory(&self, relative: &Path) -> Result<bool> {
        let (_, present) = checked_parent(self.home, relative)?;
        Ok(present && metadata(&self.home.join(relative), true)?.is_some())
    }
    fn walk(&mut self, relative: &Path, depth: usize) -> Result<()> {
        if self.budget.exhausted || self.warning.is_some() {
            return Ok(());
        }
        if depth > MAX_DEPTH {
            self.warning = Some(Warning::ScanLimit);
            return Ok(());
        }
        if !self.directory(relative)? {
            return Ok(());
        }
        #[cfg(test)]
        super::tests::directory_visit();
        for entry in fs::read_dir(self.home.join(relative)).map_err(Error::state_io_from)? {
            if !self.visit() {
                break;
            }
            let entry = entry.map_err(Error::state_io_from)?;
            let path = relative.join(entry.file_name());
            if path.to_str().is_none() {
                self.warning = Some(Warning::UnsafePath);
                break;
            }
            if entry.file_type().map_err(Error::state_io_from)?.is_dir() {
                self.walk(&path, depth + 1)?;
            } else {
                self.file(&path)?;
            }
        }
        Ok(())
    }
    fn todos(&mut self, session: &str) -> Result<()> {
        let root = Path::new("todos");
        if self.budget.exhausted || self.warning.is_some() {
            return Ok(());
        }
        let mut agents = BTreeSet::from([session.to_owned()]);
        // The session walk already proved these direct child transcript paths.
        // No enumeration of the shared todos directory is needed or permitted.
        for path in self.files.keys() {
            let parts: Vec<_> = path.iter().collect();
            if parts.len() != 5
                || parts[0] != "projects"
                || parts[2] != session
                || parts[3] != "subagents"
            {
                continue;
            }
            if let Some(id) = parts[4]
                .to_str()
                .and_then(|name| name.strip_prefix("agent-"))
                .and_then(|name| name.strip_suffix(".jsonl"))
            {
                if id.is_empty()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                {
                    self.warning = Some(Warning::UnsafePath);
                    return Ok(());
                }
                agents.insert(id.to_owned());
            }
        }
        for agent in agents {
            let name = format!("{session}-agent-{agent}.json");
            // Both supported native filesystems limit a basename to 255 bytes.
            // A legal child transcript basename can derive a longer todo name.
            if name.len() > 255 {
                self.warning = Some(Warning::UnsafePath);
                return Ok(());
            }
            self.file(&root.join(name))?;
            if self.budget.exhausted || self.warning.is_some() {
                break;
            }
        }
        Ok(())
    }
}
