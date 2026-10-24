//! The audio side of the encoder pipe.
//!
//! `FFmpeg` already takes the video frames on stdin, so the mixed audio
//! travels over a Windows named pipe instead: one instance, one reader,
//! created before the engine starts so the path in its arguments exists.
//! The engine opens that path as an ordinary raw-PCM input, and closing
//! our handle is what tells it the stream ended.

use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};

/// `PIPE_ACCESS_OUTBOUND`: this end only writes.
const ACCESS_OUTBOUND: u32 = 0x0000_0002;
/// `PIPE_REJECT_REMOTE_CLIENTS`: only a local process may read the stream.
const REJECT_REMOTE: u32 = 0x0000_0008;
/// `ERROR_PIPE_CONNECTED`: the client connected before we asked; success.
const PIPE_CONNECTED: i32 = 535;
/// `ERROR_BROKEN_PIPE`: the reader is gone.
const BROKEN_PIPE: i32 = 109;
/// Size of each pipe buffer, in bytes; a few hundred milliseconds of audio.
const BUFFER_BYTES: u32 = 256 * 1024;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        instances: u32,
        out_bytes: u32,
        in_bytes: u32,
        timeout_ms: u32,
        security: *mut c_void,
    ) -> *mut c_void;
    fn ConnectNamedPipe(pipe: *mut c_void, overlapped: *mut c_void) -> i32;
    fn WriteFile(
        file: *mut c_void,
        buffer: *const u8,
        bytes: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetCurrentProcessId() -> u32;
}

/// Distinct name per recording, so a stale instance can never be picked up.
static NEXT: AtomicU32 = AtomicU32::new(1);

/// A live named-pipe server for one recording's audio.
pub struct Server {
    /// The server end; closed on drop, which the engine reads as EOF.
    handle: *mut c_void,
    /// UTF-16 path handed to the engine, kept for the argument string.
    path: String,
}

// The handle is a kernel object owned by this value; the audio worker is
// the only thread that ever uses it, it is moved there once at start, and
// pipe handles may be used from any thread.
unsafe impl Send for Server {}

impl Server {
    /// Create an instance nobody is connected to yet; call [`Self::path`] to
    /// name it in the engine's arguments, then [`Self::connect`].
    ///
    /// # Errors
    /// Fails when Windows refuses the instance (usually a name collision).
    pub fn create() -> io::Result<Self> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = format!(
            r"\\.\pipe\screencapture-{}-{id}",
            // SAFETY: parameterless query of this process's own id.
            unsafe { GetCurrentProcessId() }
        );
        let name = crate::winapi::wide(&path);
        // SAFETY: the name is a live, nul-terminated string; the mode bits
        // describe a local, synchronous, byte-mode server with one reader.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                ACCESS_OUTBOUND,
                REJECT_REMOTE,
                1,
                BUFFER_BYTES,
                0,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == (-1_isize as *mut c_void) || handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle, path })
    }

    /// The path to pass to the engine as a raw-PCM input.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Wait for the engine to open the pipe.
    ///
    /// The engine opens its inputs right after it starts, so this returns in
    /// milliseconds; a build that refuses the arguments instead exits without
    /// ever connecting, which the recorder's stop budget then bounds.
    ///
    /// # Errors
    /// Fails when Windows refuses the connection.
    pub fn connect(&mut self) -> io::Result<()> {
        // SAFETY: a live instance created by this value.
        if unsafe { ConnectNamedPipe(self.handle, std::ptr::null_mut()) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        // A reader that connected between creation and this call reports
        // itself this way, and the stream is usable.
        if error.raw_os_error() == Some(PIPE_CONNECTED) {
            return Ok(());
        }
        Err(error)
    }

    /// Write every byte, looping over what the pipe accepted.
    ///
    /// # Errors
    /// Fails when the reader has gone away.
    pub fn write_all(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
            let mut written = 0_u32;
            // SAFETY: the buffer and its length are live for the call, and
            // the out slot is a local.
            let ok = unsafe {
                WriteFile(
                    self.handle,
                    data.as_ptr(),
                    length,
                    &raw mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                let detail = if error.raw_os_error() == Some(BROKEN_PIPE) {
                    io::Error::other("the engine closed the audio stream")
                } else {
                    error
                };
                return Err(detail);
            }
            if written == 0 {
                return Err(io::Error::other("the audio pipe accepted no bytes"));
            }
            data = &data[usize::try_from(written).unwrap_or(data.len())..];
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the handle was created by this value and is dropped
            // once; closing it is the engine's end-of-stream signal.
            unsafe {
                CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}
