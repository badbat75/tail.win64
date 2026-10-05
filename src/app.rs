//! Glue between the parsed command line, the inputs and the output.

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use crate::cli::{self, Exit, FollowMode, Settings};
use crate::follow::{FollowConfig, Watch, follow};
use crate::input::{self, Operand, Source};
use crate::output::{Headers, Output};
use crate::tail::{Delimiter, tail_seekable, tail_stream};
use crate::{describe, report, sys};

/// Why processing one operand stopped.
enum Failure {
    /// Writing to stdout failed: nothing else can be printed, stop everything.
    Output(io::Error),
    /// This input failed; report it and carry on (possibly still following it).
    Input(String, Option<Box<Watch>>),
}

/// Runs `tail` with the given `argv` (program name included).
pub fn run<I>(argv: I) -> ExitCode
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    let settings = match cli::parse(argv) {
        Ok(settings) => settings,
        Err(Exit::Info(text)) => {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(Exit::Usage(text)) => {
            eprint!("{text}");
            return ExitCode::FAILURE;
        }
    };
    for warning in &settings.warnings {
        report(format_args!("warning: {warning}"));
    }

    let operands = input::operands(settings.files.clone());
    let mut headers = Headers::new(settings.headers.unwrap_or(operands.len() > 1));
    let mut out = Output::stdout(settings.delimiter == Delimiter::AnyNewline);
    let mut ok = true;
    let mut watches = Vec::new();

    for (index, operand) in operands.into_iter().enumerate() {
        match tail_operand(index, operand, &settings, &mut out, &mut headers) {
            Ok(watch) => watches.extend(watch),
            Err(Failure::Output(e)) => return output_failed(&e),
            Err(Failure::Input(message, watch)) => {
                report(message);
                ok = false;
                watches.extend(watch.map(|w| *w));
            }
        }
    }
    if let Err(e) = out.flush() {
        return output_failed(&e);
    }

    if let Some(mode) = settings.follow {
        if watches.is_empty() {
            if !ok {
                report("no files remaining");
            }
        } else {
            let config = FollowConfig { mode, retry: settings.retry, sleep: settings.sleep, pid: settings.pid };
            match follow(watches, &config, &mut out, &mut headers) {
                Ok(all_present) => ok &= all_present,
                Err(e) => return output_failed(&e),
            }
        }
    }

    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Prints the requested part of one operand and returns what to follow, if anything.
fn tail_operand(
    index: usize,
    operand: Operand,
    settings: &Settings,
    out: &mut Output,
    headers: &mut Headers,
) -> Result<Option<Watch>, Failure> {
    let (count, delim) = (settings.count, settings.delimiter);
    let name = operand.name;

    let path = match operand.source {
        Source::Path(path) => path,
        Source::Stdin => {
            headers.switch_to(index, &name, out).map_err(Failure::Output)?;
            let Some(mut file) = sys::stdin_regular_file() else {
                // A pipe or a console: it ends at EOF, so there is nothing to follow.
                let result = tail_stream(&mut io::stdin().lock(), count, delim, out);
                return result.map(|_| None).map_err(|e| read_failure(e, out, &name));
            };
            let result = tail_seekable(&mut file, count, delim, out);
            let pos = result.map_err(|e| read_failure(e, out, &name))?;
            return Ok(match settings.follow {
                Some(FollowMode::Descriptor) => Some(Watch::open(index, name, None, file, pos)),
                Some(FollowMode::Name) => {
                    report("warning: cannot follow '-' by name");
                    None
                }
                None => None,
            });
        }
    };

    let retry_later = || {
        let wanted = settings.follow.is_some() && settings.retry;
        wanted.then(|| Box::new(Watch::missing(index, name.clone(), path.clone())))
    };
    if path.is_dir() {
        return Err(Failure::Input(format!("error reading '{name}': Is a directory"), None));
    }
    let mut file = sys::open_shared(&path)
        .map_err(|e| Failure::Input(format!("cannot open '{name}' for reading: {}", describe(&e)), retry_later()))?;
    headers.switch_to(index, &name, out).map_err(Failure::Output)?;

    // Devices and named pipes (`NUL`, `\\.\pipe\x`) cannot be scanned backwards.
    if !sys::is_regular_file(&file) {
        let result = tail_stream(&mut file, count, delim, out);
        return result.map(|_| None).map_err(|e| read_failure(e, out, &name));
    }
    let result = tail_seekable(&mut file, count, delim, out);
    let pos = result.map_err(|e| read_failure(e, out, &name))?;
    Ok(settings.follow.map(|_| Watch::open(index, name, Some(path), file, pos)))
}

/// The algorithms mix reads and writes; the output tracks which side failed.
fn read_failure(e: io::Error, out: &Output, name: &str) -> Failure {
    if out.write_failed() {
        Failure::Output(e)
    } else {
        Failure::Input(format!("error reading '{name}': {}", describe(&e)), None)
    }
}

fn output_failed(e: &io::Error) -> ExitCode {
    // The reader went away (`tail big.log | Select-Object -First 1`): that is
    // a normal way for a pipeline to end, not an error worth reporting.
    if e.kind() == io::ErrorKind::BrokenPipe {
        return ExitCode::SUCCESS;
    }
    report(format_args!("write error: {}", describe(e)));
    let _ = io::stderr().flush();
    ExitCode::FAILURE
}
