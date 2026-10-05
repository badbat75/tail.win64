//! `--follow`: keep printing data appended to the inputs.
//!
//! The implementation polls every `--sleep-interval`, like GNU tail does when
//! inotify is unavailable. Polling is predictable on every Windows file system
//! (including SMB shares, where change notifications are unreliable) and the
//! cost of one handle query per file per second is negligible.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::cli::FollowMode;
use crate::output::Headers;
use crate::sys::{self, FileId};
use crate::tail::{BUF_SIZE, read_some};
use crate::{describe, report};

#[derive(Debug, Clone, Copy)]
pub struct FollowConfig {
    pub mode: FollowMode,
    pub retry: bool,
    pub sleep: Duration,
    pub pid: Option<u32>,
}

/// One followed input.
pub struct Watch {
    index: usize,
    name: String,
    /// `None` for standard input, which can only be followed by descriptor.
    path: Option<PathBuf>,
    file: Option<File>,
    id: Option<FileId>,
    pos: u64,
    /// Permanently dropped from the follow set.
    gone: bool,
}

impl Watch {
    /// An input that was opened and printed; following resumes at `pos`.
    pub fn open(index: usize, name: String, path: Option<PathBuf>, file: File, pos: u64) -> Self {
        let id = sys::file_id(&file).ok();
        Watch { index, name, path, file: Some(file), id, pos, gone: false }
    }

    /// An input that could not be opened yet (only useful with `--retry`).
    pub fn missing(index: usize, name: String, path: PathBuf) -> Self {
        Watch { index, name, path: Some(path), file: None, id: None, pos: 0, gone: false }
    }

    /// Prints whatever was appended since the last call. Returns true if
    /// anything was written. Only output errors are returned as `Err`.
    fn drain(&mut self, buf: &mut [u8], out: &mut dyn Write, headers: &mut Headers) -> io::Result<bool> {
        let Some(file) = self.file.as_mut() else { return Ok(false) };
        // The handle (not the path) is queried: NTFS updates the size stored in
        // the directory entry lazily, the handle always sees the real one.
        let len = match file.metadata() {
            Ok(meta) => meta.len(),
            Err(e) => return Ok(self.read_failed(&e)),
        };
        if len < self.pos {
            report(format_args!("{}: file truncated", self.name));
            self.pos = 0;
        }
        if len == self.pos {
            return Ok(false);
        }
        if let Err(e) = file.seek(SeekFrom::Start(self.pos)) {
            return Ok(self.read_failed(&e));
        }
        let mut wrote = false;
        loop {
            let n = match read_some(file, buf) {
                Ok(0) => return Ok(wrote),
                Ok(n) => n,
                Err(e) => return Ok(self.read_failed(&e) || wrote),
            };
            if !wrote {
                headers.switch_to(self.index, &self.name, out)?;
                wrote = true;
            }
            out.write_all(&buf[..n])?;
            self.pos += n as u64;
        }
    }

    fn read_failed(&mut self, err: &io::Error) -> bool {
        report(format_args!("error reading '{}': {}", self.name, describe(err)));
        self.file = None;
        false
    }

    /// Checks whether the path still names the file we hold open.
    fn recheck(&mut self, config: &FollowConfig) {
        let Some(path) = self.path.as_ref() else {
            // Standard input cannot be reopened.
            self.gone = self.file.is_none();
            return;
        };
        let opened = sys::open_shared(path).and_then(|f| Ok((sys::file_id(&f)?, f)));
        match opened {
            Ok((id, file)) if self.file.is_none() => {
                report(format_args!("'{}' has appeared;  following new file", self.name));
                self.replace(file, id);
            }
            Ok((id, file)) if self.id != Some(id) => {
                report(format_args!("'{}' has been replaced;  following new file", self.name));
                self.replace(file, id);
            }
            Ok(_) => {}
            Err(e) => {
                if self.file.take().is_some() {
                    report(format_args!("'{}' has become inaccessible: {}", self.name, describe(&e)));
                }
                if !config.retry {
                    self.gone = true;
                }
            }
        }
    }

    fn replace(&mut self, file: File, id: FileId) {
        self.file = Some(file);
        self.id = Some(id);
        self.pos = 0;
    }
}

/// Follows `watches` until `--pid` dies or nothing is left to follow.
/// Returns `Ok(false)` when it stopped because every file went away.
pub fn follow(
    mut watches: Vec<Watch>,
    config: &FollowConfig,
    out: &mut dyn Write,
    headers: &mut Headers,
) -> io::Result<bool> {
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        // Sample the PID first so data written just before it exits is still shown.
        let writer_done = config.pid.is_some_and(|pid| !sys::process_alive(pid));
        let mut wrote = false;
        for watch in watches.iter_mut().filter(|w| !w.gone) {
            wrote |= watch.drain(&mut buf, out, headers)?;
            let needs_recheck = match config.mode {
                FollowMode::Name => true,
                // By descriptor, --retry only covers a file that was never opened.
                FollowMode::Descriptor => watch.file.is_none() && watch.id.is_none() && config.retry,
            };
            if needs_recheck {
                watch.recheck(config);
                // Pick up what the new file already contains.
                wrote |= watch.drain(&mut buf, out, headers)?;
            } else if watch.file.is_none() {
                watch.gone = true;
            }
        }
        if wrote {
            out.flush()?;
        }
        if writer_done {
            return Ok(true);
        }
        if watches.iter().all(|w| w.gone) {
            report("no files remaining");
            return Ok(false);
        }
        std::thread::sleep(config.sleep);
    }
}
