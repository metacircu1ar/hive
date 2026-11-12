//! Persist direct native locations; never discover them by searching a provider home.
use super::{files, queries};
use crate::{
    config::Provider,
    error::{Error, Result},
    runtime::{domain::Agent, store::Store},
};
use rusqlite::params;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

super::super::rows::record! {
    pub(super) struct Location {
        pub path: String = 0,
        pub verified: bool = 1,
    }
}

impl Store {
    pub(crate) fn claude_transcript(cwd: &Path, thread: &str) -> Option<PathBuf> {
        Some(claude_project(cwd)?.join(format!("{thread}.jsonl")))
    }

    pub(crate) fn claude_branch_paths(&self, source: &Agent, thread: &str) -> Result<Vec<PathBuf>> {
        let paths: Vec<String> = self.sql().cached().query_column(
            queries::OWNED_CLAUDE_PATHS,
            params![source.id, source.auth_generation.to_string(), thread],
        )?;
        if paths.len() > 128 {
            return Err(crate::runtime::branching::history::refused());
        }
        let mut paths: std::collections::BTreeSet<PathBuf> =
            paths.into_iter().map(PathBuf::from).collect();
        if let Some(path) = Self::claude_transcript(&source.cwd, thread) {
            paths.insert(path);
        }
        Ok(paths.into_iter().collect())
    }
    /// The launcher supplies its current realpath, before releasing native exec.
    /// Claude's transcript may be elsewhere on resume, so retain every location.
    pub(crate) fn record_claude_cwd(&self, agent: &Agent, thread: &str, cwd: &Path) -> Result<()> {
        self.record_native_binding(agent, thread)?;
        if agent.profile.provider == Provider::Claude
            && let Some(project) = claude_project(cwd)
        {
            self.save_native_path(
                agent,
                thread,
                &project.join(format!("{thread}.jsonl")),
                false,
            )?;
        }
        Ok(())
    }

    /// A provider reply or the existing Claude history verifier supplies this path.
    /// The leaf need not exist yet. Missing optional metadata never prevents startup.
    pub(crate) fn record_native_path(
        &self,
        agent: &Agent,
        thread: &str,
        path: &Path,
    ) -> Result<()> {
        self.record_native_binding(agent, thread)?;
        let home = self.native_home(agent.auth_generation)?;
        let canonical_home = home.canonicalize().map_err(Error::state_io_from)?;
        let relative = path
            .strip_prefix(&canonical_home)
            .or_else(|_| path.strip_prefix(&home));
        if let Ok(relative) = relative {
            self.save_native_path(agent, thread, relative, true)?;
        }
        Ok(())
    }

    fn save_native_path(
        &self,
        agent: &Agent,
        thread: &str,
        path: &Path,
        verified: bool,
    ) -> Result<()> {
        if !files::transcript_path(agent.profile.provider, thread, path) {
            return Ok(());
        }
        let Some(path) = path.to_str() else {
            return Ok(());
        };
        self.execute_cached(
            queries::RECORD_PATH,
            params![
                agent.id,
                agent.auth_generation.to_string(),
                agent.profile.provider.as_str(),
                thread,
                path,
                verified,
            ],
        )?;
        Ok(())
    }
}

/// Claude's native project encoder: NFC after realpath, UTF-16 substitutions,
/// then its signed 32-bit string hash for names longer than 200 code units.
fn claude_project(cwd: &Path) -> Option<PathBuf> {
    let normalized: String = cwd.to_str()?.nfc().collect();
    let mut name = String::new();
    let mut hash = 0_i32;
    for unit in normalized.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
        name.push(match char::from_u32(u32::from(unit)) {
            Some(character) if character.is_ascii_alphanumeric() => character,
            _ => '-',
        });
    }
    if name.len() > 200 {
        name.truncate(200);
        name.push('-');
        name.push_str(&base36(hash.unsigned_abs()));
    }
    Some(Path::new("projects").join(name))
}

fn base36(mut value: u32) -> String {
    let mut digits = Vec::new();
    loop {
        let digit = (value % 36) as u8;
        digits.push(char::from(if digit < 10 {
            b'0' + digit
        } else {
            b'a' + digit - 10
        }));
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_project_names_use_native_nfc_utf16_and_long_name_rules() {
        assert_eq!(
            claude_project(Path::new("/caf\u{e9}/\u{1f600}")),
            Some(PathBuf::from("projects/-caf----"))
        );
        assert_eq!(
            claude_project(Path::new("/cafe\u{301}/\u{1f600}")),
            claude_project(Path::new("/caf\u{e9}/\u{1f600}"))
        );
        let cwd = format!("/{}\u{1f600}\u{e9}", "x".repeat(210));
        // Native Int32 hashing includes both surrogate code units of the emoji.
        assert_eq!(
            claude_project(Path::new(&cwd)),
            Some(PathBuf::from(format!(
                "projects/-{}-i2epsn",
                "x".repeat(199)
            )))
        );
        assert_eq!(base36(i32::MIN.unsigned_abs()), "zik0zk");
    }
}
