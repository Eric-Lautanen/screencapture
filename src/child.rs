//! Child processes: `FFmpeg` and `ffprobe` are console programs, and Windows
//! hands a console child its own console window unless told not to, which
//! flashes a terminal on screen every time a capture is saved.

use std::process::Command;

#[cfg(windows)]
pub fn quiet(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    /// `CREATE_NO_WINDOW` (`winbase.h`): no console for the child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn quiet(_command: &mut Command) {}
