//! Idle working-set release after a capture finishes.
//!
//! A still photo leaves two full-size blocks behind: the capture frame and
//! the bitmap the system clipboard now holds on our behalf. Neither is a
//! leak — the frame goes with its target, and the clipboard copy lives
//! until another application copies — but while their pages stay resident
//! Task Manager reports the app as several megabytes heavier than it is
//! when newly started, which reads as the app growing after every capture.
//!
//! Asking Windows to trim this process's working set makes those pages
//! non-resident without freeing them: the clipboard keeps pasting exactly
//! what was copied, and the next touch pages the bytes back in. The call is
//! a hint the memory manager may ignore; when it honors it, the idle
//! footprint comes back at once instead of whenever Windows trims on its
//! own.

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn SetProcessWorkingSetSize(
        process: *mut std::ffi::c_void,
        minimum: usize,
        maximum: usize,
    ) -> i32;
}

/// Ask Windows to page out what this process is not using.
///
/// Only for an idle process: trimming during a recording would evict the
/// capture buffers and the next frame would have to fault them back in.
#[cfg(windows)]
pub(crate) fn idle() {
    // `(SIZE_T)-1` for both bounds is the documented "let the system
    // decide" form; the process pseudo-handle needs no release.
    const LET_WINDOWS_DECIDE: usize = usize::MAX;
    // SAFETY: the sentinel sizes and this process's own pseudo-handle are
    // exactly what the call documents.
    let _ = unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), LET_WINDOWS_DECIDE, LET_WINDOWS_DECIDE)
    };
}

/// Other platforms hand their resident pages back on their own schedule;
/// nothing here asks for it.
#[cfg(not(windows))]
pub(crate) const fn idle() {}

#[cfg(all(test, windows))]
mod tests {
    use super::idle;

    /// A cheap fold that changes if any byte does.
    fn checksum(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0_u64, |sum, byte| {
            sum.wrapping_mul(31).wrapping_add(u64::from(*byte))
        })
    }

    /// Trimming is a residency hint, never a release: the bytes the process
    /// still owns — the clipboard bitmap above all — must read back exactly
    /// as they were written.
    #[test]
    fn trimming_keeps_every_byte_the_process_owns() {
        let mut block = vec![0_u8; 32 * 1024 * 1024];
        for (index, byte) in block.iter_mut().enumerate() {
            *byte = u8::try_from(index % 251).expect("a modulus under 256 fits");
        }
        let expected = checksum(&block);
        std::hint::black_box(&block);
        idle();
        assert_eq!(checksum(&block), expected, "the trim must not alter memory");
        assert_eq!(block.len(), 32 * 1024 * 1024);
    }
}
