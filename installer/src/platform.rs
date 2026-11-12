//! Optional native parts can have a higher OS minimum than the installer.
use crate::{Result, options::Args};

#[cfg(target_os = "macos")]
const VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) struct Selection {
    pub(crate) parts: Vec<&'static str>,
    pub(crate) skipped_llama: bool,
}

#[cfg_attr(
    target_os = "linux",
    expect(
        clippy::unnecessary_wraps,
        reason = "Keep the same selection API as macOS, where the native version probe can fail"
    )
)]
pub(crate) fn selected(args: &Args) -> Result<Selection> {
    let parts = args.parts();
    #[cfg(target_os = "macos")]
    if parts.contains(&crate::manifest::LLAMA_COMPONENT) {
        let output = crate::native::capture(
            "/usr/bin/sw_vers",
            &["-productVersion".into()],
            VERSION_TIMEOUT,
        )?;
        if output.code != 0 {
            return Err("Cannot determine macOS version for the optional bundled server.".into());
        }
        let selection = for_macos(parts, output.stdout.trim())?;
        if selection.skipped_llama {
            println!("Skipping comb-llama: its bundled server requires macOS 13.3 or newer.");
        }
        return Ok(selection);
    }
    Ok(Selection {
        parts,
        skipped_llama: false,
    })
}

#[cfg(any(target_os = "macos", test))]
fn for_macos(mut parts: Vec<&'static str>, version: &str) -> Result<Selection> {
    let fields = version
        .split('.')
        .map(str::parse::<u16>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !(2..=3).contains(&fields.len()) {
        return Err("Unsupported macOS version response.".into());
    }
    let skipped_llama =
        (fields[0], fields[1]) < (13, 3) && parts.contains(&crate::manifest::LLAMA_COMPONENT);
    if skipped_llama {
        parts.retain(|part| *part != crate::manifest::LLAMA_COMPONENT);
    }
    Ok(Selection {
        parts,
        skipped_llama,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_macos_only_skips_the_optional_server() {
        for version in ["11.0", "12.7.6", "13.2.9"] {
            let selected = for_macos(vec!["hive", "comb", "comb-llama"], version).unwrap();
            assert!(selected.skipped_llama);
            assert_eq!(selected.parts, ["hive", "comb"]);
        }
        for version in ["13.3", "13.3.1", "14.0", "26.1"] {
            let selected = for_macos(vec!["comb", "comb-llama"], version).unwrap();
            assert!(!selected.skipped_llama);
            assert_eq!(selected.parts, ["comb", "comb-llama"]);
        }
        for version in ["", "13", "13.x", "13.3.1.2"] {
            assert!(for_macos(vec!["comb", "comb-llama"], version).is_err());
        }
    }
}
