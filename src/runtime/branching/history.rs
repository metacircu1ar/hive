//! Verify the actual inherited dialogue at each native Claude branch.
mod cohort;
mod record;
use crate::{
    error::{Error, Result},
    state,
};
use record::{Header, Kind};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
};
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime) struct Dialogue {
    pub id: String,
    parent: Option<String>,
    kind: Kind,
    message: Value,
}
pub(in crate::runtime) fn refused() -> Error {
    Error::unsupported_operation(
        "This Claude branch could not verify its copied conversation history; the \
            branch was refused.",
    )
}
struct LocatedHistory {
    path: PathBuf,
    rows: Vec<Dialogue>,
}
enum Link {
    Parent(Option<String>),
    Invalid,
}
impl Link {
    fn record(
        header: &Header<'_>,
        ids: &mut HashSet<String>,
        links: &mut BTreeMap<String, Self>,
    ) -> Result<()> {
        if let Some(id) = header.id() {
            if !ids.insert(id.to_owned()) {
                return Err(refused());
            }
            // Metadata is not dialogue. Validate its parent if the main graph
            // actually traverses it; duplicate IDs are always refused.
            links.insert(id.to_owned(), Self::read(header));
        }
        Ok(())
    }
    fn read(header: &Header<'_>) -> Self {
        match header.identity() {
            Ok((_, parent)) => Self::Parent(parent.map(str::to_owned)),
            Err(_) => Self::Invalid,
        }
    }
    fn parent(&self) -> Result<Option<&str>> {
        match self {
            Self::Parent(parent) => Ok(parent.as_deref()),
            Self::Invalid => Err(refused()),
        }
    }
}
pub(in crate::runtime) struct Locations {
    source: Vec<PathBuf>,
    child: Vec<PathBuf>,
}
impl Locations {
    pub(in crate::runtime) fn new(source: &[PathBuf], cwd: &Path, child: &str) -> Result<Self> {
        if source.is_empty() || source.len() > 129 {
            return Err(refused());
        }
        let mut paths: BTreeSet<_> = source
            .iter()
            .map(|path| path.with_file_name(format!("{child}.jsonl")))
            .collect();
        if let Some(path) = crate::runtime::store::Store::claude_transcript(cwd, child) {
            paths.insert(path);
        }
        Ok(Self {
            source: source.to_vec(),
            child: paths.into_iter().collect(),
        })
    }
}
fn read(home: &Path, session: &str, paths: &[PathBuf]) -> Result<LocatedHistory> {
    read_optional(home, session, paths)?
        .filter(|history| !history.rows.is_empty())
        .ok_or_else(refused)
}
fn read_optional(home: &Path, session: &str, paths: &[PathBuf]) -> Result<Option<LocatedHistory>> {
    uuid::Uuid::parse_str(session).map_err(|cause| refused().with_source(cause))?;
    state::check_path(home, true)?;
    if !directory_present(&home.join("projects"))? {
        return Ok(None);
    }
    let mut found = None;
    for relative in paths.iter().collect::<BTreeSet<_>>() {
        let components: Vec<_> = relative.components().collect();
        if components.len() != 3
            || components
                .iter()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || components[0].as_os_str() != "projects"
            || relative.file_name() != Some(format!("{session}.jsonl").as_ref())
        {
            return Err(refused());
        }
        let path = home.join(relative);
        if !directory_present(path.parent().ok_or_else(refused)?)? {
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                state::check_path(path.parent().ok_or_else(refused)?, true)?;
                if found.replace(path).is_some() {
                    return Err(refused());
                }
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(Error::state_io_from(cause)),
        }
    }
    let Some(path) = found else {
        return Ok(None);
    };
    let identity = crate::binary::BinaryIdentity::read(&path)?;
    let bytes = crate::os::read_capped(state::private_reader(&path)?, MAX_BYTES + 1)
        .map_err(Error::state_io_from)?;
    if crate::convert::usize_u64(bytes.len()) > MAX_BYTES {
        return Err(refused());
    }
    // An append in progress is not evidence yet; the branch gate retries within
    // its fixed deadline without releasing tools, output or the source hold.
    if !identity.unchanged() || !bytes.ends_with(b"\n") {
        return Ok(None);
    }
    Ok(Some(LocatedHistory {
        path,
        rows: decode(&bytes)?,
    }))
}
fn directory_present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            state::check_path(path, true)?;
            Ok(true)
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(cause) => Err(Error::state_io_from(cause)),
    }
}
fn decode(bytes: &[u8]) -> Result<Vec<Dialogue>> {
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    let mut links = BTreeMap::<String, Link>::new();
    let mut cohort = cohort::Cohort::default();
    for (count, line) in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        if count >= MAX_RECORDS {
            return Err(refused());
        }
        let mut value: Value =
            serde_json::from_slice(line).map_err(|cause| refused().with_source(cause))?;
        let header = Header::read(&value)?;
        if header.sidechain {
            continue;
        }
        if matches!(header.kind, Some("progress" | "system" | "attachment")) {
            Link::record(&header, &mut ids, &mut links)?;
            continue;
        }
        let Some(kind) = Kind::read(header.kind) else {
            continue;
        };
        let (id, parent_id) = header.identity()?;
        if !header.valid_message() {
            return Err(refused());
        }
        // Ordinary dialogue follows the preceding main record through known
        // metadata. A physical copy of disconnected rows is not inherited context.
        let expected_parent = rows.last().map(|row: &Dialogue| row.id.as_str());
        // Native parallel tool results point at their own assistant blocks.
        // Only a proved current cohort can replace the ordinary linear proof.
        if !cohort.observe(&value, kind, id, parent_id)? {
            check_ancestor(parent_id, expected_parent, &links)?;
        }
        if !ids.insert(id.to_owned()) {
            return Err(refused());
        }
        let id = id.to_owned();
        let parent = parent_id.map(str::to_owned);
        let message = value
            .as_object_mut()
            .and_then(|fields| fields.remove("message"))
            .ok_or_else(refused)?;
        rows.push(Dialogue {
            id,
            parent,
            kind,
            message,
        });
    }
    Ok(rows)
}
pub(in crate::runtime) struct Snapshot {
    home: PathBuf,
    source: String,
    child: String,
    rows: Vec<Dialogue>,
    locations: Locations,
}
impl Snapshot {
    pub(in crate::runtime) fn capture(
        home: &Path,
        source: &str,
        child: &str,
        cutoff: &str,
        locations: Locations,
    ) -> Result<Self> {
        if source == child {
            return Err(refused());
        }
        let rows = read(home, source, &locations.source)?.rows;
        if !rows
            .iter()
            .any(|row| row.id == cutoff && row.kind == Kind::User)
        {
            return Err(refused());
        }
        Ok(Self {
            home: home.into(),
            source: source.into(),
            child: child.into(),
            rows,
            locations,
        })
    }
    pub(in crate::runtime) fn verify(&self, input: &str) -> Result<Option<PathBuf>> {
        if read(&self.home, &self.source, &self.locations.source)?.rows != self.rows {
            return Err(refused());
        }
        let Some(child) = read_optional(&self.home, &self.child, &self.locations.child)? else {
            return Ok(None);
        };
        if child.rows.len() <= self.rows.len() {
            return Ok(None);
        }
        extends(&self.rows, &child.rows)?;
        let first = &child.rows[self.rows.len()];
        if first.id != input || first.kind != Kind::User {
            return Err(refused());
        }
        Ok(Some(child.path))
    }
}
fn extends(before: &[Dialogue], after: &[Dialogue]) -> Result<()> {
    if after.len() > before.len() && after.starts_with(before) {
        Ok(())
    } else {
        Err(refused())
    }
}
#[cfg(test)]
mod tests;

fn check_ancestor(
    parent_id: Option<&str>,
    expected_parent: Option<&str>,
    links: &BTreeMap<String, Link>,
) -> Result<()> {
    let mut ancestor = parent_id;
    let mut visited = HashSet::new();
    while ancestor != expected_parent {
        let id = ancestor.ok_or_else(refused)?;
        if !visited.insert(id) {
            return Err(refused());
        }
        ancestor = links.get(id).ok_or_else(refused)?.parent()?;
    }
    Ok(())
}
