//! Running one tool over one file, and surviving whatever it does.
//!
//! Four upstream behaviours shape this module:
//!
//! 1. **A tool can hang for good.** `chip_srom.c` doubles a `UINT32` mask until
//!    it exceeds a ROM size read verbatim out of a data block; above
//!    `0x80000000` the mask wraps to zero and the loop never ends. So every run
//!    has a deadline and is killed at it.
//! 2. **A tool waits for a keypress on exit.** `common.h`'s `DblClickWait`
//!    calls `_getch()` whenever `argv[0]` looks like an absolute Windows path,
//!    which is exactly how a spawned child sees itself. It returns early when
//!    `MSYSTEM` names an MSYS environment, so that is what the child is given.
//! 3. **A tool prints, sometimes a lot.** `vgm_sro` reports a table with a row
//!    per ROM region. Piping that and not draining it would deadlock on a full
//!    pipe, so output goes to a file instead -- and the file is what a failure
//!    quotes.
//! 4. **`DblClickWait` is not the only `_getch`.** `vgm_ptch`'s `-StripList`
//!    pauses mid-listing (`vgm_ptch.c:200`) with a bare `_getch()` that no
//!    environment variable disarms. Nothing here invokes that command, but it
//!    is why the deadline is not optional.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::ToolOutcome;
use crate::command::{self, ToolId};
use crate::exe::Tool;

/// How long any one file gets before the run is treated as hung.
///
/// Generous enough for a multi-pass `vgm_cmp` over a large rip, short enough
/// that the infinite loop above costs a pack export one file rather than the
/// afternoon.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(120);

/// How the child ended.
pub(crate) enum Ended {
    /// It exited on its own, with this code (`None` if a signal took it).
    Exited(Option<i32>),
    /// It was still running at the deadline and has been killed.
    TimedOut,
}

/// Runs `tool` over `input`, asking it to write `output`.
///
/// Whether `output` exists afterwards is the tool's answer: the three
/// optimisers write only when the result is smaller than what they were given.
pub(crate) fn run(tool: Tool, input: &Path, output: &Path, log: &Path) -> Result<Ended, String> {
    run_args(tool, &[input.as_os_str(), output.as_os_str()], log)
}

/// Runs `tool` with exactly `args`.
///
/// The general form, because not every tool takes `<input> <output>`:
/// `vgm_ptch` patches every file named on its command line **in place**
/// (`vgm_ptch.c:283`), after a run of `-Command` flags. So its caller copies
/// the file somewhere private first and reads that back afterwards.
pub(crate) fn run_args(tool: Tool, args: &[&std::ffi::OsStr], log: &Path) -> Result<Ended, String> {
    let exe = tool
        .path()
        .map_err(|error| format!("could not unpack {}: {error}", tool.name()))?;

    let sink = std::fs::File::create(log)
        .map_err(|error| format!("could not open a log for {}: {error}", tool.name()))?;
    let sink_err = sink
        .try_clone()
        .map_err(|error| format!("could not open a log for {}: {error}", tool.name()))?;

    let mut command = Command::new(exe);
    command
        .args(args)
        // See the module note: this is upstream's own escape hatch out of
        // `DblClickWait`, and it does not care what `argv[0]` looks like.
        .env("MSYSTEM", "MSYS")
        .stdin(Stdio::null())
        .stdout(Stdio::from(sink))
        .stderr(Stdio::from(sink_err));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: without it a console flashes up for every track of
        // a pack export.
        command.creation_flags(0x0800_0000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", tool.name()))?;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Ended::Exited(status.code())),
            Ok(None) => {}
            Err(error) => return Err(format!("lost track of {}: {error}", tool.name())),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Ended::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The last few lines the tool printed, for a failure message -- read from the
/// log file and trimmed by [`command::tail`], the same policy the web applies to
/// its captured stdout.
pub(crate) fn tail(log: &Path) -> String {
    std::fs::read_to_string(log)
        .map(|text| command::tail(&text))
        .unwrap_or_default()
}

/// Turns a finished native run into a [`ToolOutcome`].
///
/// The harness-level endings -- a spawn error, the deadline, a killing signal --
/// are native-only and decided here. The exit-code interpretation defers to
/// [`command_outcome`](command::command_outcome), so the desktop path cannot
/// drift from the wasm hosts on what a code means. `read_output` is called only
/// for a clean exit and hands back the bytes the tool produced (`None` when it
/// produced nothing, or nothing that differs), or a failure of its own reading.
pub(crate) fn outcome(
    id: ToolId,
    ended: Result<Ended, String>,
    log: &Path,
    read_output: impl FnOnce() -> Result<Option<Vec<u8>>, ToolOutcome>,
) -> ToolOutcome {
    match ended {
        Err(reason) => ToolOutcome::Failed(reason),
        Ok(Ended::TimedOut) => ToolOutcome::Failed(format!(
            "{} did not finish within {}s and was stopped",
            id.name(),
            TIMEOUT.as_secs()
        )),
        Ok(Ended::Exited(None)) => ToolOutcome::Failed(format!("{} was terminated", id.name())),
        Ok(Ended::Exited(Some(0))) => match read_output() {
            Ok(output) => command::command_outcome(id, 0, output, &tail(log)),
            Err(failed) => failed,
        },
        Ok(Ended::Exited(Some(code))) => {
            let tail = tail(log);
            // A decline (the file left untouched and valid) is logged rather than
            // shown; `command_outcome` turns it into `Unchanged`.
            if id.declines_with(code) {
                log::debug!("{} left the file alone: {tail}", id.name());
            }
            command::command_outcome(id, code, None, &tail)
        }
    }
}
