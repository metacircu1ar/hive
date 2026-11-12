//! Only bounded regular payloads are ever extracted, into private staging.
use crate::{
    Result,
    manifest::{self, MAX_BYTES, Manifest},
    publication,
};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
};
#[cfg(test)]
mod tests;

struct Snapshot {
    directory: publication::Owned,
    file: fs::File,
}
struct Hashing {
    input: io::Take<fs::File>,
    hash: Sha256,
    #[cfg(test)]
    source: std::path::PathBuf,
}
impl Read for Hashing {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.input.read(output)?;
        self.hash.update(&output[..count]);
        #[cfg(test)]
        tests::during_copy(&self.source)?;
        Ok(count)
    }
}
fn checked_archive(source: &Path, staging: &Path, part: &str, target: &str) -> Result<Snapshot> {
    let name = format!("{part}-{target}.tar.gz");
    let sums = String::from_utf8(manifest::read_bounded(
        &source.join("SHA256SUMS"),
        1024 * 1024,
    )?)?;
    let matches: Vec<_> = sums
        .lines()
        .filter_map(|line| {
            let mut pieces = line.split_whitespace();
            let hash = pieces.next()?;
            let file = pieces.next()?;
            (file == name && pieces.next().is_none()).then_some(hash)
        })
        .collect();
    let path = source.join(&name);
    if matches.len() != 1 {
        return Err(format!("Missing or mismatched archive checksum: {name}").into());
    }
    let input = crate::ownership::regular(&path)?;
    let size = input.metadata()?.len();
    if size > MAX_BYTES {
        return Err("Oversized release archive".into());
    }
    let directory = publication::Owned::new(
        tempfile::Builder::new()
            .prefix("archive-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(staging)?,
    )?;
    let file = directory.path().join("verified.tar.gz");
    let result = (|| {
        let mut reader = Hashing {
            input: input.take(size + 1),
            hash: Sha256::new(),
            #[cfg(test)]
            source: path.clone(),
        };
        publication::stream(&file, 0o600, &mut reader, size, &atomic_fs::DefaultBarriers)?;
        if format!("{:x}", reader.hash.finalize()) != matches[0] {
            return Err(format!("Missing or mismatched archive checksum: {name}").into());
        }
        crate::ownership::regular(&file)
    })();
    match result {
        Ok(file) => Ok(Snapshot { directory, file }),
        Err(error) => directory.finish(Err(error)),
    }
}

pub fn unpack(source: &Path, staging: &Path, part: &str, target: &str) -> Result<Manifest> {
    let snapshot = checked_archive(source, staging, part, target)?;
    #[cfg(test)]
    tests::after_verification(&source.join(format!("{part}-{target}.tar.gz")))?;
    let root = staging.join(part);
    let result = (|| {
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        let input = GzDecoder::new(&snapshot.file).take(MAX_BYTES + 1024 * 1024);
        let names = extract(input, &root)?;
        check(&root, names, part, target)
    })();
    snapshot.directory.finish(result)
}
fn extract(input: impl Read, root: &Path) -> Result<BTreeSet<String>> {
    let mut archive = tar::Archive::new(input);
    let mut names = BTreeSet::new();
    let mut bytes = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .ok_or("Non-UTF8 archive path")?
            .to_owned();
        let size = entry.size();
        bytes = bytes.checked_add(size).ok_or("Archive size overflow")?;
        if !manifest::safe_path(&name)
            || !entry.header().entry_type().is_file()
            || !names.insert(name.clone())
            || names.len() > 129
            || bytes > MAX_BYTES
        {
            return Err("Archive contains unsafe, duplicate or oversized entries".into());
        }
        let mode = entry.header().mode()?;
        if ![0o644, 0o755].contains(&mode) {
            return Err("Unsafe payload mode".into());
        }
        let file = root.join(&name);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(file.parent().ok_or("Missing payload parent")?)?;
        publication::stream(&file, mode, &mut entry, size, &atomic_fs::DefaultBarriers)?;
    }
    Ok(names)
}
fn check(root: &Path, names: BTreeSet<String>, part: &str, target: &str) -> Result<Manifest> {
    let manifest: Manifest = hive_input::compatibility::record::decode(
        &manifest::read_bounded(&root.join("manifest.json"), 1024 * 1024)?,
        &crate::compatibility::INSTALLATION,
    )?;
    manifest.validate(target)?;
    if manifest.part.as_str() != part
        || names
            != manifest
                .files
                .keys()
                .cloned()
                .chain(["manifest.json".into()])
                .collect()
    {
        return Err("Manifest and archive payload differ".into());
    }
    manifest.check_files(root)?;
    manifest.check_versions(root)?;
    Ok(manifest)
}
