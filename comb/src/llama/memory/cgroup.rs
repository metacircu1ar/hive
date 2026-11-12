//! Resolve memory hierarchies through the process's actual mount namespace.
use crate::{Error, Result};
use std::{
    io,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    V1,
    V2,
}

struct Mount {
    kind: Kind,
    root: PathBuf,
    point: PathBuf,
}

fn path(text: &str) -> Result<PathBuf> {
    // mountinfo escapes these four bytes. Decode backslash last so a literal
    // backslash followed by octal digits is not decoded a second time.
    let path = PathBuf::from(
        text.replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    );
    if !path.is_absolute()
        || path
            .components()
            .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::llama_memory_unknown());
    }
    Ok(path)
}

impl Mount {
    fn parse(line: &str) -> Result<Option<Self>> {
        let (before, after) = line
            .split_once(" - ")
            .ok_or_else(Error::llama_memory_unknown)?;
        let mut fields = after.split_whitespace();
        let filesystem = fields.next().ok_or_else(Error::llama_memory_unknown)?;
        fields.next().ok_or_else(Error::llama_memory_unknown)?;
        let options = fields.next().ok_or_else(Error::llama_memory_unknown)?;
        let kind = match filesystem {
            "cgroup2" => Kind::V2,
            "cgroup" if options.split(',').any(|name| name == "memory") => Kind::V1,
            _ => return Ok(None),
        };
        let mut fields = before.split_whitespace().skip(3);
        let root = path(fields.next().ok_or_else(Error::llama_memory_unknown)?)?;
        let point = path(fields.next().ok_or_else(Error::llama_memory_unknown)?)?;
        Ok(Some(Self { kind, root, point }))
    }

    fn memory(&self) -> Result<bool> {
        if self.kind == Kind::V1 {
            return Ok(true);
        }
        Ok(
            std::fs::read_to_string(self.point.join("cgroup.controllers"))?
                .split_whitespace()
                .any(|name| name == "memory"),
        )
    }

    fn bound(&self, member: &Path, available: u64) -> Result<Option<u64>> {
        let Ok(relative) = member.strip_prefix(&self.root) else {
            return Ok(None);
        };
        let mut directory = self.point.join(relative);
        if !directory.is_dir() {
            return Ok(None);
        }
        let (maximum, current) = match self.kind {
            Kind::V1 => ("memory.limit_in_bytes", "memory.usage_in_bytes"),
            Kind::V2 => ("memory.max", "memory.current"),
        };
        let mut available = available;
        loop {
            match std::fs::read_to_string(directory.join(maximum)) {
                Ok(limit) if limit.trim() == "max" => (),
                Ok(limit) => {
                    let used = std::fs::read_to_string(directory.join(current))?;
                    let used = number(&used)?;
                    let cache = self.file_cache(&directory)?;
                    let working = used.saturating_sub(cache);
                    available = available.min(number(&limit)?.saturating_sub(working));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
            if directory == self.point {
                return Ok(Some(available));
            }
            if !directory.pop() {
                return Err(Error::llama_memory_unknown());
            }
        }
    }

    fn file_cache(&self, directory: &Path) -> Result<u64> {
        let text = std::fs::read_to_string(directory.join("memory.stat"))?;
        let (active, inactive) = match self.kind {
            Kind::V1 => ("total_active_file", "total_inactive_file"),
            Kind::V2 => ("active_file", "inactive_file"),
        };
        // File LRU pages are reclaimable, including the just-hashed GGUF.
        // These counters exclude shmem/tmpfs, unlike the broad `file` counter.
        stat(&text, active)?
            .checked_add(stat(&text, inactive)?)
            .ok_or_else(Error::llama_memory_unknown)
    }
}

fn stat(text: &str, key: &str) -> Result<u64> {
    let mut rows = text
        .lines()
        .map(str::split_whitespace)
        .filter(|row| row.clone().next() == Some(key));
    let mut row = rows.next().ok_or_else(Error::llama_memory_unknown)?;
    row.next();
    let value = number(row.next().ok_or_else(Error::llama_memory_unknown)?)?;
    if row.next().is_some() || rows.next().is_some() {
        return Err(Error::llama_memory_unknown());
    }
    Ok(value)
}

fn number(text: &str) -> Result<u64> {
    text.trim()
        .parse()
        .map_err(|cause| Error::llama_memory_unknown().caused_by(cause))
}

fn membership(line: &str) -> Result<Option<(Kind, PathBuf)>> {
    let mut fields = line.splitn(3, ':');
    fields.next().ok_or_else(Error::llama_memory_unknown)?;
    let controllers = fields.next().ok_or_else(Error::llama_memory_unknown)?;
    let member = fields.next().ok_or_else(Error::llama_memory_unknown)?;
    let kind = if controllers.is_empty() {
        Kind::V2
    } else if controllers.split(',').any(|name| name == "memory") {
        Kind::V1
    } else {
        return Ok(None);
    };
    Ok(Some((kind, path(member)?)))
}

pub(super) fn bound(proc: &Path, mut available: u64) -> Result<u64> {
    let text = std::fs::read_to_string(proc.join("self/mountinfo"))?;
    let mounts: Vec<_> = text
        .lines()
        .map(Mount::parse)
        .filter_map(std::result::Result::transpose)
        .collect::<Result<_>>()?;
    for line in std::fs::read_to_string(proc.join("self/cgroup"))?.lines() {
        let Some((kind, member)) = membership(line)? else {
            continue;
        };
        let (mut memory, mut resolved) = (false, false);
        for mount in mounts.iter().filter(|mount| mount.kind == kind) {
            if !mount.memory()? {
                continue;
            }
            memory = true;
            if let Some(bound) = mount.bound(&member, available)? {
                available = bound;
                resolved = true;
            }
        }
        // Hybrid v2 without memory is unrelated. A mounted memory hierarchy
        // must resolve, and a declared v1 memory controller cannot be ignored.
        if memory && !resolved || kind == Kind::V1 && !memory {
            return Err(Error::llama_memory_unknown());
        }
    }
    Ok(available)
}
