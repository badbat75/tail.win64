//! FILE operands: wildcard expansion and display names.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Stdin,
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operand {
    /// Name used in headers and messages, as GNU tail prints it.
    pub name: String,
    pub source: Source,
}

impl Operand {
    fn from_arg(arg: OsString) -> Self {
        if arg == "-" {
            return Operand { name: "standard input".into(), source: Source::Stdin };
        }
        Operand { name: arg.to_string_lossy().into_owned(), source: Source::Path(arg.into()) }
    }
}

/// Turns the FILE arguments into operands; no arguments means standard input.
///
/// Neither `cmd.exe` nor PowerShell expand wildcards for native programs, so
/// `tail *.log` would otherwise look for a file literally named `*.log`. A
/// pattern is expanded here unless a file with that exact name exists; a
/// pattern without matches is kept literally so the user gets "cannot open".
pub fn operands(args: Vec<OsString>) -> Vec<Operand> {
    if args.is_empty() {
        return vec![Operand::from_arg("-".into())];
    }
    args.into_iter().flat_map(expand).collect()
}

fn expand(arg: OsString) -> Vec<Operand> {
    let pattern = match arg.to_str() {
        Some(s) if s.contains(['*', '?', '[']) && !PathBuf::from(s).exists() => s,
        _ => return vec![Operand::from_arg(arg)],
    };
    let options = glob::MatchOptions { case_sensitive: !cfg!(windows), ..Default::default() };
    let mut matches: Vec<PathBuf> = match glob::glob_with(pattern, options) {
        Ok(paths) => paths.filter_map(Result::ok).filter(|p| !p.is_dir()).collect(),
        Err(_) => Vec::new(),
    };
    if matches.is_empty() {
        return vec![Operand::from_arg(arg)];
    }
    matches.sort();
    matches.into_iter().map(|p| Operand::from_arg(p.into_os_string())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_is_stdin() {
        assert_eq!(operands(vec![]), vec![Operand { name: "standard input".into(), source: Source::Stdin }]);
    }

    #[test]
    fn unmatched_pattern_is_kept() {
        let ops = operands(vec!["surely-not-here-*.xyz".into()]);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "surely-not-here-*.xyz");
    }

    #[test]
    fn pattern_expands_sorted() {
        let dir = std::env::temp_dir().join(format!("tail-win-glob-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b.log", "a.log", "c.txt"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let pattern = dir.join("*.log").into_os_string();
        let names: Vec<_> = operands(vec![pattern])
            .into_iter()
            .map(|o| PathBuf::from(o.name).file_name().unwrap().to_owned())
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, vec![OsString::from("a.log"), OsString::from("b.log")]);
    }
}
