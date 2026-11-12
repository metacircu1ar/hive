//! Kernel birth and lock-file identities; saved PIDs are never signal authority.
// Saved birth identities contain the platform tag and its native time components.
use super::{corrupt, private_file, recovery::recovery_required};
use crate::{
    error::{Error, Result},
    os,
};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::{fs, io::ErrorKind};
use std::{os::fd::AsFd, path::Path, process::id as process_id, str};
use uuid::Uuid;
#[cfg(target_os = "linux")]
const LINUX_BIRTH_COMPONENTS: usize = 2;
#[cfg(target_os = "macos")]
const MACOS_BIRTH_COMPONENTS: usize = 3;
// Kernel process birth times express their fractional second in microseconds.
#[cfg(target_os = "macos")]
const MICROSECONDS_PER_SECOND: u64 = 1_000_000;
// /proc stat field 22, after stripping PID and the parenthesized command fields.
#[cfg(target_os = "linux")]
const START_TIME_FIELD: usize = 19;

#[cfg(target_os = "linux")]
pub fn boot_identity() -> Result<String> {
    parse_os_boot_identity(
        &fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(Error::state_io_from)?,
    )
}

#[cfg(target_os = "macos")]
pub fn boot_identity() -> Result<String> {
    let os::BootIdentityBytes {
        bytes: boot,
        length: size,
    } = os::boot_identity_bytes().map_err(Error::state_io_from)?;
    let id = str::from_utf8(&boot[..size.min(boot.len())])
        .map_err(|cause| corrupt().with_source(cause))?
        .trim_end_matches('\0');
    parse_os_boot_identity(id)
}

fn parse_os_boot_identity(value: &str) -> Result<String> {
    // Normalize the OS boundary (including Linux's trailing newline) before
    // persisting a boot identity. Saved records always contain UUID text.
    Uuid::parse_str(value.trim())
        .map(|id| id.to_string())
        .map_err(|cause| corrupt().with_source(cause))
}

pub fn same_boot(saved: &str) -> Result<bool> {
    let saved = Uuid::parse_str(saved).map_err(|cause| corrupt().with_source(cause))?;
    let current =
        Uuid::parse_str(&boot_identity()?).map_err(|cause| corrupt().with_source(cause))?;
    Ok(saved == current)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub birth: String,
}
impl ProcessIdentity {
    pub fn current() -> Result<Self> {
        Self::read(process_id())?.ok_or_else(recovery_required)
    }
    pub fn absent(&self) -> Result<bool> {
        self.validate()?;
        Ok(Self::read(self.pid)?.as_ref() != Some(self))
    }
    fn validate(&self) -> Result<()> {
        let parts = self.birth.split(':').collect::<Vec<_>>();
        let numeric = |s: &str| {
            !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_digit())
                && s.parse::<u64>().is_ok_and(|n| n.to_string() == s)
        };
        #[cfg(target_os = "linux")]
        let valid =
            parts.len() == LINUX_BIRTH_COMPONENTS && parts[0] == "linux" && numeric(parts[1]);
        #[cfg(target_os = "macos")]
        let valid = parts.len() == MACOS_BIRTH_COMPONENTS
            && parts[0] == "macos"
            && numeric(parts[1])
            && numeric(parts[2])
            && parts[2]
                .parse::<u64>()
                .is_ok_and(|n| n < MICROSECONDS_PER_SECOND);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let valid = false;
        if valid { Ok(()) } else { Err(corrupt()) }
    }
    pub fn read(pid: u32) -> Result<Option<Self>> {
        if pid == 0 || pid > i32::MAX.unsigned_abs() {
            return Err(recovery_required());
        }
        #[cfg(target_os = "linux")]
        {
            Self::from_linux_stat(pid, fs::read_to_string(format!("/proc/{pid}/stat")))
        }
        #[cfg(target_os = "macos")]
        {
            Ok(os::process_birth(pid).map_err(Error::state_io_from)?.map(
                |os::ProcessBirth {
                     seconds,
                     microseconds,
                 }| Self {
                    pid,
                    birth: format!("macos:{seconds}:{microseconds}"),
                },
            ))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(recovery_required())
        }
    }

    #[cfg(target_os = "linux")]
    fn from_linux_stat(pid: u32, observed: std::io::Result<String>) -> Result<Option<Self>> {
        let text = match observed {
            Ok(text) => text,
            // An opened proc file returns ESRCH if its task is reaped before read.
            Err(e) if e.kind() == ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) => {
                return Ok(None);
            }
            Err(e) => return Err(Error::state_io_from(e)),
        };
        let fields = text
            .rsplit_once(')')
            .ok_or_else(corrupt)?
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        let ticks = fields
            .get(START_TIME_FIELD)
            .ok_or_else(corrupt)?
            .parse::<u64>()
            .map_err(|cause| corrupt().with_source(cause))?;
        Ok(Some(Self {
            pid,
            birth: format!("linux:{ticks}"),
        }))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LockIdentity {
    dev: u64,
    ino: u64,
}
impl LockIdentity {
    pub fn from_fd(fd: impl AsFd) -> Result<Self> {
        let os::FileIdentity {
            device: dev,
            inode: ino,
        } = os::file_identity(fd).map_err(Error::state_io_from)?;
        Ok(Self { dev, ino })
    }
    pub(super) fn at(path: &Path) -> Result<Self> {
        Self::from_fd(private_file(path, false)?)
    }
    /// Only for a saved observation, after validating the containing namespace.
    /// Equality between two live observations deliberately remains exact.
    pub(super) fn matches_saved(&self, live: &Self, parent: &Path) -> Result<bool> {
        use std::os::unix::fs::MetadataExt;
        if self == live {
            return Ok(true);
        }
        super::check_path(parent, true)?;
        let parent = std::fs::symlink_metadata(parent).map_err(Error::state_io_from)?;
        Ok(hive_setup_support::files::identity::saved(
            (self.dev, self.ino),
            (live.dev, live.ino),
            parent.dev(),
        )
        .is_match())
    }
    pub(super) fn matches_on_boot(&self, live: &Self, parent: &Path, boot: &str) -> Result<bool> {
        if same_boot(boot)? {
            Ok(self == live)
        } else {
            self.matches_saved(live, parent)
        }
    }
}

#[cfg(test)]
#[test]
fn ownership_identities_require_exact_fields() {
    super::assert_strict_record::<ProcessIdentity>(
        &serde_json::json!({"pid": 1, "birth": "macos:1:0"}),
        &[],
    );
    super::assert_strict_record::<LockIdentity>(&serde_json::json!({"dev": 1, "ino": 1}), &[]);
}

#[cfg(test)]
#[test]
fn os_boot_ids_are_canonicalized_before_persistence() {
    let id = Uuid::from_u128(0x1234_abcd);
    for input in [
        id.to_string(),
        format!("{}\n", id.to_string().to_uppercase()),
    ] {
        assert_eq!(
            parse_os_boot_identity(&input).expect("OS UUID"),
            id.to_string()
        );
    }
    for input in [
        "",
        "not-a-uuid\n",
        "00000000-0000-0000-0000-000000000000 trailing",
    ] {
        assert_eq!(
            parse_os_boot_identity(input)
                .expect_err("malformed OS UUID")
                .code(),
            "state_inconsistent"
        );
    }
    let current = boot_identity().expect("current boot");
    assert!(same_boot(&current).expect("canonical stored boot"));
    assert!(same_boot(&format!("{current}\n")).is_err());
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn process_identity_handles_reaping_between_proc_open_and_read() {
    use std::{io::Read, process::Command};
    let mut command = Command::new("/bin/sleep");
    command.arg("60");
    let mut child = crate::test_support::ChildGuard(os::spawn(&mut command).expect("spawn"));
    let pid = child.id();
    let mut stat = fs::File::open(format!("/proc/{pid}/stat")).expect("open live proc stat");
    let identity = ProcessIdentity::read(pid)
        .expect("live identity")
        .expect("running child");
    child.kill().expect("kill owned child");
    child.wait().expect("reap owned child");
    let mut contents = String::new();
    let error = stat
        .read_to_string(&mut contents)
        .expect_err("reaped proc file");
    assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
    assert!(
        ProcessIdentity::from_linux_stat(pid, Err(error))
            .expect("gone")
            .is_none()
    );
    assert!(identity.absent().expect("original process is absent"));
    for code in [libc::EACCES, libc::EPERM, libc::EIO, libc::EMFILE] {
        assert!(
            ProcessIdentity::from_linux_stat(pid, Err(std::io::Error::from_raw_os_error(code)))
                .is_err()
        );
    }
    assert!(ProcessIdentity::from_linux_stat(pid, Ok("malformed".into())).is_err());
}
