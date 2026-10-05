//! Parsing of the `NUM` argument of `-n` / `-c` (GNU semantics).
//!
//! Accepted forms: `[+|-]DIGITS[SUFFIX]`. A leading `+` means "starting from
//! unit NUM", anything else means "the last NUM units". Values that overflow
//! `u64` saturate, exactly like GNU tail does.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Lines,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Output the last `n` units.
    FromEnd,
    /// Output starting with unit number `n` (1-based).
    FromStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub unit: Unit,
    pub origin: Origin,
    pub n: u64,
}

impl Default for Count {
    fn default() -> Self {
        Count { unit: Unit::Lines, origin: Origin::FromEnd, n: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseCountError(pub String);

impl fmt::Display for ParseCountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Parses a full `-n`/`-c` argument such as `+5`, `-2K`, `10`.
pub fn parse_count(arg: &str, unit: Unit) -> Result<Count, ParseCountError> {
    let (origin, digits) = match arg.as_bytes().first() {
        Some(b'+') => (Origin::FromStart, &arg[1..]),
        Some(b'-') => (Origin::FromEnd, &arg[1..]),
        _ => (Origin::FromEnd, arg),
    };
    let what = match unit {
        Unit::Lines => "number of lines",
        Unit::Bytes => "number of bytes",
    };
    let n = parse_size(digits).ok_or_else(|| ParseCountError(format!("invalid {what}: '{arg}'")))?;
    Ok(Count { unit, origin, n })
}

/// Parses `DIGITS[SUFFIX]` into a saturating `u64`.
///
/// Suffixes follow GNU coreutils: `b`=512, `kB`=1000, `K`/`KiB`=1024, and so on
/// through `E`, `Z`, `Y`, `R`, `Q` (the large ones always saturate).
pub fn parse_size(s: &str) -> Option<u64> {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, suffix) = s.split_at(split);
    if digits.is_empty() {
        return None;
    }
    let base = digits.bytes().fold(0u64, |acc, d| acc.saturating_mul(10).saturating_add(u64::from(d - b'0')));
    Some(base.saturating_mul(suffix_multiplier(suffix)?))
}

fn suffix_multiplier(suffix: &str) -> Option<u64> {
    if suffix.is_empty() {
        return Some(1);
    }
    if suffix == "b" {
        return Some(512);
    }
    let mut chars = suffix.chars();
    let letter = chars.next()?;
    let power = match letter {
        'k' | 'K' => 1,
        'm' | 'M' => 2,
        'g' | 'G' => 3,
        't' | 'T' => 4,
        'p' | 'P' => 5,
        'e' | 'E' => 6,
        'z' | 'Z' => 7,
        'y' | 'Y' => 8,
        'r' | 'R' => 9,
        'q' | 'Q' => 10,
        _ => return None,
    };
    // GNU only accepts lowercase for `k` (and `b`); other lowercase letters are invalid.
    if letter.is_ascii_lowercase() && letter != 'k' {
        return None;
    }
    let base: u64 = match chars.as_str() {
        "" | "iB" => 1024,
        "B" => 1000,
        _ => return None,
    };
    Some((0..power).fold(1u64, |acc, _| acc.saturating_mul(base)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_numbers() {
        assert_eq!(parse_size("0"), Some(0));
        assert_eq!(parse_size("42"), Some(42));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_size("1x"), None);
    }

    #[test]
    fn suffixes() {
        assert_eq!(parse_size("2b"), Some(1024));
        assert_eq!(parse_size("1K"), Some(1024));
        assert_eq!(parse_size("1k"), Some(1024));
        assert_eq!(parse_size("1KiB"), Some(1024));
        assert_eq!(parse_size("1kB"), Some(1000));
        assert_eq!(parse_size("3M"), Some(3 * 1024 * 1024));
        assert_eq!(parse_size("1MB"), Some(1_000_000));
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("1m"), None);
        assert_eq!(parse_size("1Kb"), None);
    }

    #[test]
    fn saturation() {
        assert_eq!(parse_size("1Y"), Some(u64::MAX));
        assert_eq!(parse_size("99999999999999999999999"), Some(u64::MAX));
    }

    #[test]
    fn signs() {
        let c = parse_count("+5", Unit::Lines).unwrap();
        assert_eq!((c.origin, c.n), (Origin::FromStart, 5));
        let c = parse_count("-5", Unit::Bytes).unwrap();
        assert_eq!((c.origin, c.n, c.unit), (Origin::FromEnd, 5, Unit::Bytes));
        let c = parse_count("7", Unit::Lines).unwrap();
        assert_eq!((c.origin, c.n), (Origin::FromEnd, 7));
        assert!(parse_count("+", Unit::Lines).is_err());
        assert!(parse_count("--5", Unit::Lines).is_err());
    }
}
