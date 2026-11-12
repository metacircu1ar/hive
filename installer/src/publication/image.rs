//! Pin the executing image and its notices before installed pointers can change.
use crate::{Result, manifest, ownership};
use std::{
    fs,
    io::{Seek, SeekFrom},
    path::Path,
};
#[cfg(test)]
mod tests;

pub(crate) struct Image {
    executable: fs::File,
    expected: manifest::File,
    notices: Vec<u8>,
}
impl Image {
    pub(crate) fn capture() -> Result<Self> {
        Self::at(&std::env::current_exe()?.canonicalize()?)
    }
    fn at(path: &Path) -> Result<Self> {
        let mut executable = ownership::regular(path)?;
        let expected = manifest::File {
            bytes: executable.metadata()?.len(),
            sha256: manifest::hash_file(&mut executable)?,
            mode: 0o755,
        };
        #[cfg(not(test))]
        let notices = {
            let mut sibling = path.as_os_str().to_owned();
            sibling.push(".LICENSES");
            manifest::read_bounded(Path::new(&sibling), 16 * 1024 * 1024).map_err(|cause| {
                super::context(
                    "Installer notices must accompany its concrete executable as NAME.LICENSES",
                    cause,
                )
            })?
        };
        #[cfg(test)]
        let notices = b"Installer test fixture notices".to_vec();
        Ok(Self {
            executable,
            expected,
            notices,
        })
    }
    pub(crate) fn copy(&self, root: &Path) -> Result<(manifest::File, manifest::File)> {
        let binary = root.join("bin/hive-install");
        let mut reader = self.executable.try_clone()?;
        reader.seek(SeekFrom::Start(0))?;
        super::stream(
            &binary,
            0o755,
            &mut reader,
            self.expected.bytes,
            &atomic_fs::DefaultBarriers,
        )?;
        manifest::check_payload(&binary, &self.expected)?;
        let notices = root.join("bin/hive-install.LICENSES");
        super::write(&notices, &self.notices)?;
        let saved = manifest::File {
            bytes: manifest::byte_count(self.notices.len()),
            sha256: manifest::hash(&notices)?,
            mode: 0o644,
        };
        Ok((self.expected.clone(), saved))
    }
}
