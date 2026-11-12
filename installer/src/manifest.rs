//! Strict current manifests, bounded files and native platform checks.
use crate::Result;
mod part;
mod probes;
pub use part::Part;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    process::Command,
};

fn validate_product_version(value: &str) -> Result<()> {
    if value.len() > 128 {
        return Err("Oversized product version".into());
    }
    semver::Version::parse(value)?;
    Ok(())
}

/// Comb's bundled llama-server: installed and removed with Comb, never alone.
pub(crate) const LLAMA_COMPONENT: &str = "comb-llama";

/// Components retain product names; maintenance invokes their CLI executable.
/// The llama-server component has no maintenance command and no owned data.
pub(crate) fn maintenance_binary(component: &str) -> Option<&str> {
    match component {
        "hive" => Some("hivectl"),
        LLAMA_COMPONENT => None,
        other => Some(other),
    }
}

pub const MAX_BYTES: u64 = 512 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub sha256: String,
    pub bytes: u64,
    pub mode: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: crate::compatibility::Format,
    pub product_version: String,
    pub part: Part,
    pub target: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub required_glibc: Option<String>,
    #[serde(deserialize_with = "hive_input::decode::object_map")]
    pub files: BTreeMap<String, File>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub format: crate::compatibility::Format,
    pub target: String,
    #[serde(deserialize_with = "hive_input::decode::object_map")]
    pub parts: BTreeMap<String, Manifest>,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub installer: File,
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub installer_notices: File,
    #[serde(deserialize_with = "hive_input::decode::map")]
    pub retired: BTreeMap<String, std::path::PathBuf>,
}

pub(crate) fn installed(bytes: &[u8]) -> Result<Installed> {
    Ok(hive_input::compatibility::record::decode_with(
        bytes,
        |value| {
            use hive_input::compatibility::record::classify;
            classify(value, &crate::compatibility::INSTALLATION)?;
            if let Some(parts) = value.get("parts").and_then(serde_json::Value::as_object) {
                for part in parts.values() {
                    classify(part, &crate::compatibility::INSTALLATION)?;
                }
            }
            Ok(())
        },
    )?)
}

pub fn native_target() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        arch => return Err(format!("Unsupported CPU: {arch}").into()),
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        os => {
            return Err(format!(
                "Unsupported operating system: {os}; only macOS and GNU/Linux are supported"
            )
            .into());
        }
    };
    Ok(format!("{arch}-{os}"))
}
pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read_file(path, limit).map_err(|cause| {
        asset(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            cause,
        )
    })
}
fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = crate::ownership::regular(path)?;
    if file.metadata()?.len() > limit {
        return Err("Oversized release metadata".into());
    }
    read_from(file, limit)
}
pub(crate) fn read_from(file: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if crate::manifest::byte_count(bytes.len()) > limit {
        return Err("Oversized release metadata".into());
    }
    Ok(bytes)
}
pub(crate) fn asset(name: String, cause: crate::Error) -> crate::Error {
    AssetError { name, cause }.into()
}
#[derive(Debug)]
pub(crate) struct AssetError {
    name: String,
    cause: crate::Error,
}
impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Release asset {}: {}", self.name, self.cause)
    }
}
impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
pub fn hash(path: &Path) -> Result<String> {
    hash_file(&mut crate::ownership::regular(path)?)
}
pub(crate) fn hash_file(input: &mut fs::File) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub(crate) fn check_payload(path: &Path, expected: &File) -> Result<()> {
    let mut file = crate::ownership::owned(path)?;
    let meta = file.metadata()?;
    if meta.len() != expected.bytes
        || meta.mode() & 0o7777 != expected.mode
        || hash_file(&mut file)? != expected.sha256
    {
        return Err("Owned payload was modified".into());
    }
    crate::ownership::named(path, &meta)
}
pub fn safe_path(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && name
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
pub fn version(value: &str) -> Result<Vec<u32>> {
    let pieces: Vec<u32> = value
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    if !(2..=3).contains(&pieces.len()) {
        return Err("Invalid GLIBC requirement".into());
    }
    Ok(pieces)
}
impl Manifest {
    pub fn component(&self) -> Result<&'static str> {
        match self.part {
            Part::Hive | Part::Core => Ok("hive"),
            Part::Comb => Ok("comb"),
            Part::CombLlama => Ok(LLAMA_COMPONENT),
            Part::Unknown(_) => Err("Unknown release part".into()),
        }
    }
    pub fn binaries(&self) -> Result<&'static [&'static str]> {
        Ok(match self.component()? {
            "hive" => &["hivectl", "hive-daemon"],
            LLAMA_COMPONENT => &["comb-llama-server"],
            _ => &["comb"],
        })
    }
    pub fn validate(&self, target: &str) -> Result<()> {
        validate_product_version(&self.product_version)?;
        if self.target != target || self.files.len() > 128 {
            return Err("Wrong release target or file count".into());
        }
        let prefix = format!("share/{}/", self.component()?);
        let binaries: BTreeSet<String> = self
            .binaries()?
            .iter()
            .map(|n| format!("bin/{n}"))
            .collect();
        for path in binaries.iter().cloned().chain([
            format!("{prefix}THIRD_PARTY_LICENSES"),
            format!("{prefix}build.json"),
        ]) {
            if !self.files.contains_key(&path) {
                return Err(format!("Required payload missing: {path}").into());
            }
        }
        let mut bytes = 0_u64;
        for (path, file) in &self.files {
            if !safe_path(path)
                || (!binaries.contains(path) && !path.starts_with(&prefix))
                || file.mode
                    != if binaries.contains(path) {
                        0o755
                    } else {
                        0o644
                    }
                || file.sha256.len() != 64
                || !file
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err(format!("Invalid release payload: {path}").into());
            }
            bytes = bytes
                .checked_add(file.bytes)
                .ok_or("Payload size overflow")?;
        }
        if bytes > MAX_BYTES {
            return Err("Release payload exceeds limit".into());
        }
        self.validate_glibc(target)?;
        Ok(())
    }
    fn validate_glibc(&self, target: &str) -> Result<()> {
        if target.contains("linux") {
            let needed = version(
                self.required_glibc
                    .as_deref()
                    .ok_or("Missing GLIBC requirement")?,
            )?;
            let text = probes::run(
                Command::new("getconf").arg("GNU_LIBC_VERSION"),
                probes::TIMEOUT,
            )?;
            let current = text
                .trim()
                .strip_prefix("glibc ")
                .ok_or("GNU/Linux with glibc required")?;
            if version(current)? < needed || version(current)? < vec![2, 35] {
                return Err(format!(
                    "glibc >= 2.35 and >= {} required; found {current}",
                    self.required_glibc.as_deref().unwrap_or("")
                )
                .into());
            }
        } else if self.required_glibc.is_some() {
            return Err("GLIBC requirement on a non-Linux artifact".into());
        }
        Ok(())
    }

    pub fn check_files(&self, root: &Path) -> Result<()> {
        crate::ownership::directory(root, crate::ownership::Directory::Inner)?;
        for (name, expected) in &self.files {
            let path = root.join(name);
            // Check each ancestor too: an edited installation cannot redirect hard links.
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if directory == root {
                    break;
                }
                crate::ownership::directory(directory, crate::ownership::Directory::Inner)?;
                parent = directory.parent();
            }
            check_payload(&path, expected)?;
        }
        Ok(())
    }
    pub fn check_versions(&self, root: &Path) -> Result<()> {
        let suffix = match self.part {
            Part::Hive => "(comb)",
            Part::Core => "(core)",
            Part::Comb => "(http)",
            // llama-server reports its version on stderr; a successful run is the
            // loader check, and packaging already bound the pinned commit.
            Part::CombLlama => "",
            Part::Unknown(_) => return Err("Unknown part".into()),
        };
        for name in self.binaries()? {
            let path = root.join("bin").join(name);
            // Running the actual payload makes the platform loader check every
            // versioned symbol against the exact host libraries it resolves.
            let text = probes::run(Command::new(&path).arg("--version"), probes::TIMEOUT).map_err(
                |error| {
                    crate::publication::context(
                        "Cannot execute payload. If temporary storage is noexec, set TMPDIR to a \
                    private directory on an executable filesystem",
                        error,
                    )
                },
            )?;
            if !text.trim().ends_with(suffix) {
                return Err("Installed variant/loader check failed.".into());
            }
        }
        Ok(())
    }
}

/// Cap a native byte/count representation at `u64::MAX` without wrapping.
/// This is exact on the supported 32-bit and 64-bit pointer widths.
#[inline]
pub(crate) fn byte_count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod count_tests {
    #[test]
    fn native_counts_preserve_zero_small_and_pointer_maximum() {
        assert_eq!(super::byte_count(0), 0);
        assert_eq!(super::byte_count(17), 17);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(super::byte_count(usize::MAX), u64::MAX);
        #[cfg(target_pointer_width = "32")]
        assert_eq!(super::byte_count(usize::MAX), u64::from(u32::MAX));
    }
}
