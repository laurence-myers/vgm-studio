//! Borrowing the parent process's console on Windows.
//!
//! `vgmstudio.exe` is linked as a GUI-subsystem executable in release builds (see
//! the crate attribute in `bin/vgmstudio.rs`) so double-clicking it does not flash
//! a console window. The cost is that a release build started *from* a console
//! has no stdio attached, and every `println!` from a subcommand would vanish.
//!
//! [`attach_parent_console`] borrows the parent's console for the rest of the
//! process, which is all the standard library needs: its Windows stdio re-queries
//! `GetStdHandle` on every write, so there is no cached handle -- and no C
//! runtime `FILE*` -- to rebind afterwards. Call it before the first print.
//!
//! Note the standing wart: an interactive shell does not *wait* for a
//! GUI-subsystem process, so the prompt returns immediately and the subcommand's
//! output interleaves with it. Redirection and pipes are unaffected.

// The workspace denies (not forbids) `unsafe_code` precisely so a module like
// this one can opt out. Attaching a console is four FFI calls with no safe
// wrapper in the ecosystem worth the dependency.
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};

/// The three standard handles, in the order they are saved and restored.
const STD_HANDLES: [STD_HANDLE; 3] = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];

/// Attaches this process to its parent's console, if it has one.
///
/// Does nothing when the process was launched from Explorer (no parent console)
/// or already has one -- in both cases `AttachConsole` fails and the standard
/// handles are left exactly as they were, so this is safe to call unconditionally.
///
/// Handles the parent redirected (`vgmstudio convert a.dro > out.txt`) are saved
/// first and put back afterwards: attaching a console overwrites the standard
/// handle slots with the console's own, which would otherwise send output to the
/// terminal that the user had asked to go to a file.
pub fn attach_parent_console() {
    // SAFETY: `GetStdHandle` with a documented STD_*_HANDLE identifier is a pure
    // read of this process's handle table; it returns a sentinel rather than
    // failing.
    let saved = STD_HANDLES.map(|id| unsafe { GetStdHandle(id) });

    // SAFETY: `AttachConsole` takes a process id; ATTACH_PARENT_PROCESS is the
    // documented sentinel for "my parent". A failure (no parent console, or one
    // already attached) is reported in the return value, not by misbehaving.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }

    for (id, handle) in STD_HANDLES.into_iter().zip(saved) {
        // A null handle means the parent gave us nothing to preserve; the
        // invalid sentinel means `GetStdHandle` itself failed. In both cases the
        // console's fresh handle is the better one to keep.
        if is_usable(handle) {
            // SAFETY: `handle` came from `GetStdHandle` for this same identifier
            // moments ago and has not been closed, so it is valid to install.
            unsafe { SetStdHandle(id, handle) };
        }
    }
}

fn is_usable(handle: HANDLE) -> bool {
    !handle.is_null() && handle != INVALID_HANDLE_VALUE
}

/// Points this process's standard output at the null device, best-effort.
///
/// The native file dialog (rfd's Windows Common Item Dialog) hosts the real
/// Explorer shell namespace, so right-clicking a file loads whatever third-party
/// context-menu shell extensions the machine has installed *in-process*. Some of
/// those write stray lines -- "dark mode 1" is the reported one -- to the
/// process's stdout, which is visible in a console-subsystem debug build.
///
/// The GUI never writes to stdout itself (its own logging goes to stderr via
/// `env_logger`), so redirecting `STD_OUTPUT_HANDLE` to `NUL` before any dialog
/// can load silences that third-party noise without losing anything of ours.
/// Windows stdio -- the standard library's included -- re-queries `GetStdHandle`
/// on every write, so pointing the slot at `NUL` covers any writer that resolves
/// stdout the same way, not just our own. The null handle is deliberately leaked:
/// it must outlive every write for the rest of the process.
///
/// A no-op if `NUL` cannot be opened; there is nothing to fall back to and the
/// stray line is cosmetic, so a failure is simply left alone.
pub fn silence_stdout() {
    use std::os::windows::io::AsRawHandle;

    let Ok(nul) = std::fs::OpenOptions::new().write(true).open("NUL") else {
        return;
    };
    let handle = nul.as_raw_handle() as HANDLE;
    // SAFETY: `handle` is a live write handle to the null device from the open
    // above; `SetStdHandle` with the documented `STD_OUTPUT_HANDLE` identifier
    // installs it into this process's handle table.
    unsafe { SetStdHandle(STD_OUTPUT_HANDLE, handle) };
    // Keep the handle alive for the process: dropping the `File` would close it
    // and leave the stdout slot dangling.
    std::mem::forget(nul);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test` runs console-subsystem test binaries that already own a
    /// console, so the call is a no-op here -- but it must not crash, and the
    /// standard handles must survive it. (The real path is only exercised by a
    /// release GUI-subsystem build; the smoke tests cover that end.)
    #[test]
    fn attaching_is_harmless_when_a_console_already_exists() {
        let before = STD_HANDLES.map(|id| unsafe { GetStdHandle(id) });
        attach_parent_console();
        let after = STD_HANDLES.map(|id| unsafe { GetStdHandle(id) });
        assert_eq!(before, after);
    }

    #[test]
    fn the_sentinels_are_not_usable_handles() {
        assert!(!is_usable(std::ptr::null_mut()));
        assert!(!is_usable(INVALID_HANDLE_VALUE));
    }
}
