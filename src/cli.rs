//! Command line parsing: turns `argv` into validated [`Settings`].

use std::ffi::OsString;
use std::time::Duration;

use clap::Parser;

use crate::count::{Count, Origin, Unit, parse_count, parse_size};
use crate::tail::Delimiter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowMode {
    /// Keep reading the same open file, even if it is renamed (`-f`).
    Descriptor,
    /// Re-open the path when it is replaced, e.g. by log rotation (`-F`).
    Name,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub count: Count,
    pub follow: Option<FollowMode>,
    pub retry: bool,
    pub sleep: Duration,
    pub pid: Option<u32>,
    /// `Some(true)` = always print headers, `Some(false)` = never, `None` = only for several files.
    pub headers: Option<bool>,
    pub delimiter: Delimiter,
    pub files: Vec<OsString>,
    /// Non-fatal remarks about the option combination, printed by the caller.
    pub warnings: Vec<String>,
}

/// What the caller must do when parsing does not produce [`Settings`].
#[derive(Debug)]
pub enum Exit {
    /// `--help` / `--version`: print to stdout and exit successfully.
    Info(String),
    /// Usage error: print to stderr and exit with status 1.
    Usage(String),
}

#[derive(Parser, Debug)]
#[command(
    name = "tail",
    version,
    about = "Print the last 10 lines of each FILE to standard output.\n\
             With more than one FILE, precede each with a header giving the file name.\n\
             With no FILE, or when FILE is -, read standard input.",
    after_help = "NUM may have a multiplier suffix: b 512, kB 1000, K 1024, MB 1000*1000, M 1024*1024, \
                  GB, G, T, P, E (and KiB, MiB, ... as aliases of K, M, ...).\n\n\
                  With --follow (-f), tail defaults to following the file descriptor, which means that \
                  even if a tailed file is renamed, tail will continue to track its end. Use \
                  --follow=name (or -F) to track the file name instead, which is what you want for \
                  rotated log files.\n\n\
                  Unlike GNU tail, a carriage return not followed by a newline also ends a line and is \
                  printed as a newline, so progress bars (tqdm, pip, curl) show one line per update \
                  instead of overwriting each other. CRLF line endings are left untouched. Use \
                  --raw-cr for the GNU behavior."
)]
struct Args {
    /// Output the last NUM bytes; or use -c +NUM to output starting with byte NUM
    #[arg(short = 'c', long = "bytes", value_name = "[+]NUM", allow_hyphen_values = true, overrides_with = "lines")]
    bytes: Option<String>,

    /// Output appended data as the file grows; HOW is 'name' or 'descriptor' (default)
    #[arg(
        short = 'f',
        long = "follow",
        value_name = "HOW",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "descriptor",
        value_parser = ["name", "descriptor"],
    )]
    follow: Option<String>,

    /// Same as --follow=name --retry
    #[arg(short = 'F')]
    follow_name_retry: bool,

    /// Output the last NUM lines, instead of the last 10; or use -n +NUM to skip NUM-1 lines at the start
    #[arg(short = 'n', long = "lines", value_name = "[+]NUM", allow_hyphen_values = true, overrides_with = "bytes")]
    lines: Option<String>,

    /// Accepted for compatibility; this port always re-checks the file name every interval
    #[arg(long = "max-unchanged-stats", value_name = "N")]
    max_unchanged_stats: Option<String>,

    /// With -f, terminate after process ID PID dies
    #[arg(long, value_name = "PID")]
    pid: Option<u32>,

    /// Never output headers giving file names
    #[arg(short = 'q', long = "quiet", visible_alias = "silent", overrides_with = "verbose")]
    quiet: bool,

    /// Treat a lone carriage return as an ordinary byte, like GNU tail (see below)
    #[arg(long = "raw-cr")]
    raw_cr: bool,

    /// Keep trying to open a file if it is inaccessible
    #[arg(long)]
    retry: bool,

    /// With -f, sleep for approximately N seconds (default 1.0) between iterations
    #[arg(short = 's', long = "sleep-interval", value_name = "N")]
    sleep_interval: Option<String>,

    /// Always output headers giving file names
    #[arg(short = 'v', long = "verbose", overrides_with = "quiet")]
    verbose: bool,

    /// Line delimiter is NUL, not newline
    #[arg(short = 'z', long = "zero-terminated")]
    zero_terminated: bool,

    #[arg(value_name = "FILE")]
    files: Vec<OsString>,
}

/// Parses the full `argv`, including the program name.
pub fn parse<I>(argv: I) -> Result<Settings, Exit>
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    let argv = expand_obsolete_syntax(argv.into_iter().map(Into::into).collect());
    let args = Args::try_parse_from(argv).map_err(|e| match e.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => Exit::Info(e.to_string()),
        _ => Exit::Usage(e.to_string()),
    })?;
    settings_from(args).map_err(|e| Exit::Usage(format!("tail: {e}\nTry 'tail --help' for more information.\n")))
}

fn settings_from(args: Args) -> Result<Settings, String> {
    let count = match (&args.lines, &args.bytes) {
        (Some(n), _) => parse_count(n, Unit::Lines),
        (_, Some(c)) => parse_count(c, Unit::Bytes),
        (None, None) => Ok(Count::default()),
    }
    .map_err(|e| e.to_string())?;

    let follow = if args.follow_name_retry {
        Some(FollowMode::Name)
    } else {
        args.follow.as_deref().map(|how| if how == "name" { FollowMode::Name } else { FollowMode::Descriptor })
    };
    let retry = args.retry || args.follow_name_retry;

    let sleep = match &args.sleep_interval {
        None => Duration::from_secs(1),
        Some(s) => s
            .parse::<f64>()
            .ok()
            .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
            .ok_or_else(|| format!("invalid number of seconds: '{s}'"))?,
    };

    if let Some(n) = &args.max_unchanged_stats {
        parse_size(n).ok_or_else(|| format!("invalid maximum number of unchanged stats between opens: '{n}'"))?;
    }

    let mut warnings = Vec::new();
    match follow {
        None if args.retry => warnings.push("--retry ignored; --retry is useful only when following".into()),
        Some(FollowMode::Descriptor) if args.retry => {
            warnings.push("--retry only effective for the initial open".into())
        }
        _ => {}
    }
    if follow.is_none() && args.pid.is_some() {
        warnings.push("PID ignored; --pid=PID is useful only when following".into());
    }

    let headers = if args.verbose {
        Some(true)
    } else if args.quiet {
        Some(false)
    } else {
        None
    };

    Ok(Settings {
        count,
        follow,
        retry,
        sleep,
        pid: args.pid,
        headers,
        delimiter: match (args.zero_terminated, args.raw_cr) {
            (true, _) => Delimiter::Byte(0),
            (false, true) => Delimiter::Byte(b'\n'),
            (false, false) => Delimiter::AnyNewline,
        },
        files: args.files,
        warnings,
    })
}

/// Rewrites the historical syntax (`tail -20`, `tail +5 file`, `tail -3cf file`)
/// into regular options, using the same acceptance rules as GNU tail: it only
/// applies when the first argument is the only option and at most one file follows.
fn expand_obsolete_syntax(mut argv: Vec<OsString>) -> Vec<OsString> {
    let accepts = match argv.len() {
        2 => true,
        3 => argv[2].to_str().is_none_or(|s| !(s.starts_with('-') && s.len() > 1)),
        _ => false,
    };
    if !accepts {
        return argv;
    }
    if let Some(replacement) = argv[1].to_str().and_then(parse_obsolete) {
        argv.splice(1..2, replacement.into_iter().map(OsString::from));
    }
    argv
}

/// Parses one obsolete option such as `-20`, `+5`, `-2b`, `-cf`.
fn parse_obsolete(arg: &str) -> Option<Vec<String>> {
    let (origin, rest) = match arg.as_bytes().first()? {
        b'+' => (Origin::FromStart, &arg[1..]),
        // A lone "-" is stdin and "-c" needs an argument: neither is obsolete syntax.
        b'-' if !matches!(&arg[1..], "" | "c") => (Origin::FromEnd, &arg[1..]),
        _ => return None,
    };
    let digits_end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let (digits, mut flags) = rest.split_at(digits_end);
    let mut n = if digits.is_empty() { 10 } else { parse_size(digits)? };

    let mut unit = Unit::Lines;
    if let Some(c) = flags.chars().next().filter(|c| matches!(c, 'b' | 'c' | 'l')) {
        unit = if c == 'l' { Unit::Lines } else { Unit::Bytes };
        if c == 'b' {
            n = n.saturating_mul(512);
        }
        flags = &flags[1..];
    }
    let follow = flags == "f";
    if !follow && !flags.is_empty() {
        return None;
    }

    let sign = if origin == Origin::FromStart { '+' } else { '-' };
    let flag = if unit == Unit::Lines { "-n" } else { "-c" };
    let mut out = vec![flag.to_string(), format!("{sign}{n}")];
    if follow {
        out.push("-f".into());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(args: &[&str]) -> Settings {
        let argv = std::iter::once("tail").chain(args.iter().copied());
        match parse(argv) {
            Ok(s) => s,
            Err(e) => panic!("{args:?} failed: {e:?}"),
        }
    }

    fn count(unit: Unit, origin: Origin, n: u64) -> Count {
        Count { unit, origin, n }
    }

    #[test]
    fn defaults() {
        let s = parse_ok(&[]);
        assert_eq!(s.count, Count::default());
        assert_eq!(s.follow, None);
        assert_eq!(s.sleep, Duration::from_secs(1));
        assert!(s.files.is_empty());
    }

    #[test]
    fn lines_and_bytes() {
        assert_eq!(parse_ok(&["-n", "5"]).count, count(Unit::Lines, Origin::FromEnd, 5));
        assert_eq!(parse_ok(&["-n5"]).count, count(Unit::Lines, Origin::FromEnd, 5));
        assert_eq!(parse_ok(&["-n", "+5"]).count, count(Unit::Lines, Origin::FromStart, 5));
        assert_eq!(parse_ok(&["-n", "-5"]).count, count(Unit::Lines, Origin::FromEnd, 5));
        assert_eq!(parse_ok(&["--lines=2K"]).count, count(Unit::Lines, Origin::FromEnd, 2048));
        assert_eq!(parse_ok(&["-c", "3", "x"]).count, count(Unit::Bytes, Origin::FromEnd, 3));
        // The last of -n / -c wins.
        assert_eq!(parse_ok(&["-n", "1", "-c", "2"]).count, count(Unit::Bytes, Origin::FromEnd, 2));
        assert_eq!(parse_ok(&["-c", "2", "-n", "1"]).count, count(Unit::Lines, Origin::FromEnd, 1));
    }

    #[test]
    fn follow_modes() {
        let s = parse_ok(&["-f", "a.log"]);
        assert_eq!(s.follow, Some(FollowMode::Descriptor));
        assert_eq!(s.files, vec![OsString::from("a.log")]);
        assert_eq!(parse_ok(&["--follow=name", "a"]).follow, Some(FollowMode::Name));
        let s = parse_ok(&["-F", "a"]);
        assert_eq!((s.follow, s.retry), (Some(FollowMode::Name), true));
        assert_eq!(parse_ok(&["-s", "0.25", "-f"]).sleep, Duration::from_millis(250));
        assert!(parse(["tail", "-s", "-1", "-f"]).is_err());
    }

    #[test]
    fn obsolete_syntax() {
        assert_eq!(parse_ok(&["-20"]).count, count(Unit::Lines, Origin::FromEnd, 20));
        assert_eq!(parse_ok(&["+5", "file"]).count, count(Unit::Lines, Origin::FromStart, 5));
        assert_eq!(parse_ok(&["-2b", "file"]).count, count(Unit::Bytes, Origin::FromEnd, 1024));
        let s = parse_ok(&["-3cf", "file"]);
        assert_eq!(s.count, count(Unit::Bytes, Origin::FromEnd, 3));
        assert_eq!(s.follow, Some(FollowMode::Descriptor));
        let s = parse_ok(&["-f", "file"]);
        assert_eq!((s.count, s.follow), (Count::default(), Some(FollowMode::Descriptor)));
        // Not obsolete: "-" is stdin, and a second option disables the syntax.
        assert_eq!(parse_ok(&["-"]).files, vec![OsString::from("-")]);
        assert!(parse(["tail", "-20", "-v"]).is_err());
    }

    #[test]
    fn delimiter_modes() {
        assert_eq!(parse_ok(&[]).delimiter, Delimiter::AnyNewline);
        assert_eq!(parse_ok(&["--raw-cr"]).delimiter, Delimiter::Byte(b'\n'));
        assert_eq!(parse_ok(&["-z"]).delimiter, Delimiter::Byte(0));
        assert_eq!(parse_ok(&["-z", "--raw-cr"]).delimiter, Delimiter::Byte(0));
    }

    #[test]
    fn headers_last_wins() {
        assert_eq!(parse_ok(&["-q", "-v"]).headers, Some(true));
        assert_eq!(parse_ok(&["-v", "-q"]).headers, Some(false));
    }

    #[test]
    fn invalid_numbers() {
        assert!(matches!(parse(["tail", "-n", "abc"]), Err(Exit::Usage(_))));
        assert!(matches!(parse(["tail", "--help"]), Err(Exit::Info(_))));
    }
}
