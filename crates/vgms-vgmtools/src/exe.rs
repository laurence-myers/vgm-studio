//! The built tools, carried inside this binary and unpacked on first use.
//!
//! Embedding the executables keeps the app a single file to ship. They are
//! unpacked to a cache directory named after a hash of their own bytes, so a
//! rebuilt app never runs a stale tool and two versions can coexist on one
//! machine.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Which optimiser to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// `vgm_cmp` -- drops chip writes that change nothing.
    Compress,
    /// `vgm_sro` -- strips unused regions out of sample ROMs.
    SampleRom,
    /// `optdac` -- collapses long runs of identical YM2612 DAC writes.
    DacRuns,
    /// `vgm_ptch` -- edits the header; used here to strip unwritten chips.
    ///
    /// The odd one out: it patches its file **in place** rather than taking an
    /// output path, so its caller works on a copy.
    Patch,
}

#[cfg(windows)]
mod embedded {
    pub(super) const COMPRESS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vgm_cmp.exe"));
    pub(super) const SAMPLE_ROM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vgm_sro.exe"));
    pub(super) const DAC_RUNS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/optdac.exe"));
    pub(super) const PATCH: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vgm_ptch.exe"));
}

#[cfg(not(windows))]
mod embedded {
    pub(super) const COMPRESS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vgm_cmp"));
    pub(super) const SAMPLE_ROM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vgm_sro"));
    pub(super) const DAC_RUNS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/optdac"));
    pub(super) const PATCH: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vgm_ptch"));
}

impl Tool {
    /// The name this tool is known by upstream -- also what it is called on
    /// disk and in log lines.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Compress => "vgm_cmp",
            Self::SampleRom => "vgm_sro",
            Self::DacRuns => "optdac",
            Self::Patch => "vgm_ptch",
        }
    }

    /// Whether `code` is this tool's way of saying "there is nothing here for
    /// me", as opposed to "I broke" -- the shared rule in
    /// [`ToolId::declines_with`](crate::command::ToolId::declines_with),
    /// which the wasm hosts apply too. `vgm_ptch` is not a pipeline command
    /// and declines nothing.
    pub(crate) const fn declines_with(self, code: i32) -> bool {
        match self {
            Self::Compress => crate::command::ToolId::Compress.declines_with(code),
            Self::SampleRom => crate::command::ToolId::SampleRom.declines_with(code),
            Self::DacRuns => crate::command::ToolId::DacRuns.declines_with(code),
            Self::Patch => false,
        }
    }

    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Compress => embedded::COMPRESS,
            Self::SampleRom => embedded::SAMPLE_ROM,
            Self::DacRuns => embedded::DAC_RUNS,
            Self::Patch => embedded::PATCH,
        }
    }

    fn file_name(self) -> String {
        if cfg!(windows) {
            format!("{}.exe", self.name())
        } else {
            self.name().to_owned()
        }
    }

    /// The unpacked executable, written out the first time it is asked for.
    pub(crate) fn path(self) -> io::Result<&'static Path> {
        let cell = match self {
            Self::Compress => {
                static CELL: OnceLock<io::Result<PathBuf>> = OnceLock::new();
                &CELL
            }
            Self::SampleRom => {
                static CELL: OnceLock<io::Result<PathBuf>> = OnceLock::new();
                &CELL
            }
            Self::DacRuns => {
                static CELL: OnceLock<io::Result<PathBuf>> = OnceLock::new();
                &CELL
            }
            Self::Patch => {
                static CELL: OnceLock<io::Result<PathBuf>> = OnceLock::new();
                &CELL
            }
        };

        match cell.get_or_init(|| unpack(self)) {
            Ok(path) => Ok(path.as_path()),
            // `io::Error` is not `Clone`, so a failed unpack is reported again
            // rather than handed back -- the message is what matters.
            Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
        }
    }
}

fn unpack(tool: Tool) -> io::Result<PathBuf> {
    let bytes = tool.bytes();
    let dir = std::env::temp_dir().join(format!("vgms-vgmtools-{:016x}", fingerprint(bytes)));
    std::fs::create_dir_all(&dir)?;

    let exe = dir.join(tool.file_name());
    if exe.exists() {
        return Ok(exe);
    }

    // Write under a name nobody executes, then rename: a second process
    // unpacking the same tool at the same time must never see a half-written
    // executable. Rename is atomic within one directory.
    let partial = dir.join(format!(
        "{}.{}.partial",
        tool.file_name(),
        std::process::id()
    ));
    std::fs::write(&partial, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    match std::fs::rename(&partial, &exe) {
        Ok(()) => {}
        // Lost the race, which is fine -- the winner wrote the same bytes.
        Err(_) if exe.exists() => {
            let _ = std::fs::remove_file(&partial);
        }
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            return Err(error);
        }
    }
    Ok(exe)
}

/// FNV-1a over the executable's bytes: enough to tell two builds apart, and
/// not worth a dependency for more.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_unpacks_to_a_real_executable() {
        for tool in [Tool::Compress, Tool::SampleRom, Tool::DacRuns, Tool::Patch] {
            let path = tool.path().expect("unpacks");
            assert!(
                path.exists(),
                "{} is missing at {}",
                tool.name(),
                path.display()
            );
            let size = std::fs::metadata(path).expect("stat").len();
            assert!(
                size > 1024,
                "{} is implausibly small ({size} bytes)",
                tool.name()
            );
        }
    }

    #[test]
    fn every_tool_is_a_different_program() {
        // A copy-paste in the embedding would otherwise run one tool four
        // times and look like it worked.
        let mut seen = std::collections::BTreeSet::new();
        for tool in [Tool::Compress, Tool::SampleRom, Tool::DacRuns, Tool::Patch] {
            assert!(
                seen.insert(fingerprint(tool.bytes())),
                "{} embeds the same bytes as another tool",
                tool.name()
            );
        }
    }
}
