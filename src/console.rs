//! Where a printing command's output goes.
//!
//! The released Windows binary is a GUI-subsystem program, so Windows never
//! creates a console window for it. That is the point: Explorer, the Start
//! Menu, the installer's finish page and the autostart entry launch the file
//! directly, and a console-subsystem program gets a console — window and all —
//! before any of its own code runs, which is the terminal that flashed on
//! screen at every launch. The tray needs no console, and a one-shot command
//! (`--check`, `--version`, a take) takes the console of the terminal that
//! started it instead of bringing its own.

/// Point this process's output at the terminal that launched it, when it has
/// nowhere to write.
///
/// Nothing happens when the launcher passed a stream along — a pipe, a
/// redirect, or a console handed to the child — and nothing happens when there
/// is no terminal at all, which is the tray-only launch: the report is dropped
/// exactly as it was when the tray detached its console.
pub fn attach() {
    native::attach();
}

#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    /// Stream ids (`winbase.h`) for the two streams the app writes through,
    /// the negative ids the API takes spelled as the `u32` it wants.
    const STD_OUTPUT_HANDLE: u32 = (-11i32).cast_unsigned();
    const STD_ERROR_HANDLE: u32 = (-12i32).cast_unsigned();
    /// `ATTACH_PARENT_PROCESS`: the console of the process that started us.
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// The handle for one of this process's standard streams, or null
        /// when it has none.
        fn GetStdHandle(which: u32) -> *mut c_void;
        /// Fails when the process already owns a console, which is the case
        /// for a launch from a terminal that passed its own.
        fn AttachConsole(process: u32) -> i32;
    }

    pub(super) fn attach() {
        if usable(STD_OUTPUT_HANDLE) && usable(STD_ERROR_HANDLE) {
            return;
        }
        // SAFETY: the sentinel takes no pointer and the call only changes
        // which console this process is attached to.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
        // Windows fills in the streams that had nothing and leaves the rest
        // alone, so a command reading `--check > report.txt` keeps writing
        // that file even when a second stream was empty.
    }

    /// Whether this process can write to stream `which`.
    fn usable(which: u32) -> bool {
        // SAFETY: a parameterless query about this own process.
        named(unsafe { GetStdHandle(which) } as usize)
    }

    /// Whether a standard-stream handle names something to write to. A
    /// GUI-subsystem process started without a console begins with nothing:
    /// null and `-1` are the two ways Windows spells that.
    pub(super) const fn named(handle: usize) -> bool {
        handle != 0 && handle != usize::MAX
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::native::named;

    /// The empty-stream spellings are the whole reason a command reaches for a
    /// terminal at all, so both have to be recognised as nothing.
    #[test]
    fn an_empty_stream_is_recognised_in_both_spellings() {
        assert!(!named(0));
        assert!(!named(usize::MAX));
        assert!(named(0x100));
    }
}

#[cfg(not(windows))]
mod native {
    /// Off Windows there is no console subsystem to be launched without: a
    /// command started from a terminal already carries it, and the tray only
    /// exists on Windows.
    pub(super) fn attach() {}
}
