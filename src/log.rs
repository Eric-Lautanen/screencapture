//! Diagnostics ring: the tray runs without a console, so errors would
//! otherwise vanish. Recent notes stay in a small ring, and `flush_to`
//! appends them to a plain-text log in the user's media directory,
//! restarting the file when it outgrows its cap.

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// Newest-kept lines in the ring; older ones fall off the back.
const CAP: usize = 64;

/// The log file restarts past this size (rotation without history math).
const MAX_LOG_BYTES: u64 = 512 * 1024;

/// Log file name, inside whatever directory the caller picked.
pub const FILE_NAME: &str = "screencapture.log";

struct Ring {
    lines: VecDeque<String>,
}

impl Ring {
    const fn new() -> Self {
        Self {
            lines: VecDeque::new(),
        }
    }

    fn keep(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > CAP {
            self.lines.pop_front();
        }
    }

    fn take(&mut self) -> VecDeque<String> {
        std::mem::take(&mut self.lines)
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new());

/// A timed diagnostic line: written to stderr and always kept in the
/// ring so `flush_to` can persist it to disk. `message` stays plain prose;
/// the tag leads every line.
pub fn note(tag: &str, message: &str) {
    let line = format!("{} {}: {}", crate::clock::stamp(), tag, message);
    eprintln!("{tag}: {message}");
    if let Ok(mut ring) = RING.lock() {
        ring.keep(line);
    }
}

/// Move the ring into `directory/screencapture.log`, starting the file
/// over when it is past its cap. Failures stay silent: logging must
/// never turn one problem into two.
pub fn flush_to(directory: &Path) {
    let taken = RING
        .lock()
        .map_or_else(|_| VecDeque::new(), |mut ring| ring.take());
    if taken.is_empty() {
        return;
    }
    let file_path = directory.join(FILE_NAME);
    if fs::create_dir_all(directory).is_err() {
        return;
    }
    let restart = fs::metadata(&file_path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES);
    let opened = if restart {
        fs::File::create(&file_path)
    } else {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
    };
    if let Ok(mut file) = opened {
        for line in &taken {
            let _ = writeln!(file, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CAP, FILE_NAME, Ring, flush_to};
    use std::fs;

    #[test]
    fn ring_keeps_the_newest_cap_lines() {
        let mut ring = Ring::new();
        for index in 0..CAP + 5 {
            ring.keep(format!("line-{index}"));
        }
        assert_eq!(ring.lines.len(), CAP);
        assert_eq!(ring.lines.front().map(String::as_str), Some("line-5"));
        assert_eq!(ring.lines.back().map(String::as_str), Some("line-68"));
    }

    #[test]
    fn take_empties_so_a_second_flush_writes_nothing_twice() {
        let mut ring = Ring::new();
        ring.keep(String::from("once"));
        assert_eq!(1, ring.take().len());
        assert!(ring.take().is_empty());
    }

    #[test]
    fn flushing_appends_and_marks_the_file() {
        let directory =
            std::env::temp_dir().join(format!("screencapture-log-test-{}", std::process::id()));
        if directory.exists() {
            let _ = fs::remove_dir_all(&directory);
        }
        super::RING
            .lock()
            .expect("unpoisoned ring")
            .keep(format!("app marker {}", std::process::id()));
        flush_to(&directory);
        let text =
            fs::read_to_string(directory.join(FILE_NAME)).expect("log file must be readable");
        assert!(text.contains(&format!("app marker {}", std::process::id())));
        let _ = fs::remove_dir_all(&directory);
    }
}
