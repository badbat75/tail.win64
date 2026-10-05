//! A native Windows port of GNU `tail`.
//!
//! Layering, from pure to OS-specific:
//! * [`count`], [`tail`]: option values and algorithms, no I/O assumptions.
//! * [`cli`], [`input`], [`output`]: command line, operands, stdout/console.
//! * [`follow`], [`app`]: `--follow` loop and orchestration.
//! * [`sys`]: every Win32 call (and the only `unsafe` code).

pub mod app;
pub mod cli;
pub mod count;
pub mod follow;
pub mod input;
pub mod output;
pub mod sys;
pub mod tail;

pub use app::run;

use std::fmt::Display;
use std::io;

/// Prints a diagnostic to stderr, prefixed like GNU tools do.
pub(crate) fn report(message: impl Display) {
    eprintln!("tail: {message}");
}

/// Describes an I/O error with the short wording GNU tail uses, instead of the
/// long Windows sentence followed by "(os error N)".
pub(crate) fn describe(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "No such file or directory".into(),
        io::ErrorKind::PermissionDenied => "Permission denied".into(),
        io::ErrorKind::IsADirectory => "Is a directory".into(),
        _ => {
            let text = err.to_string();
            let text = text.split(" (os error").next().unwrap_or(&text);
            text.trim_end().trim_end_matches('.').to_string()
        }
    }
}
