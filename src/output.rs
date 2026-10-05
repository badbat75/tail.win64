//! Standard output handling and `==> name <==` headers.

use std::io::{self, BufWriter, IsTerminal, Write};

use crate::tail::BUF_SIZE;

/// The process' standard output.
///
/// * Redirected to a file or pipe: bytes are passed through untouched.
/// * Attached to a Windows console: Rust writes through `WriteConsoleW` and
///   fails on invalid UTF-8, so the stream is sanitized first (invalid
///   sequences become U+FFFD). A log with one bad byte must not abort `tail`.
///
/// It also remembers whether a *write* failed, so callers can tell a fatal
/// output error (e.g. closed pipe) from a per-file read error.
pub struct Output {
    inner: Box<dyn Write>,
    write_failed: bool,
}

impl Output {
    /// `cr_to_lf`: print a lone `\r` as `\n` (see [`CrToLf`]).
    pub fn stdout(cr_to_lf: bool) -> Self {
        let stdout = io::stdout();
        let mut inner: Box<dyn Write> = if stdout.is_terminal() {
            Box::new(Utf8Lossy::new(BufWriter::with_capacity(BUF_SIZE, stdout.lock())))
        } else {
            Box::new(BufWriter::with_capacity(BUF_SIZE, stdout.lock()))
        };
        if cr_to_lf {
            inner = Box::new(CrToLf::new(inner));
        }
        Output { inner, write_failed: false }
    }

    pub fn write_failed(&self) -> bool {
        self.write_failed
    }

    fn track<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.write_failed = true;
        }
        result
    }
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let r = self.inner.write(buf);
        self.track(r)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let r = self.inner.write_all(buf);
        self.track(r)
    }

    fn flush(&mut self) -> io::Result<()> {
        let r = self.inner.flush();
        self.track(r)
    }
}

/// Replaces invalid UTF-8 with U+FFFD. Incomplete sequences at the end of a
/// write are held back until the next write completes (or fails to) them.
pub struct Utf8Lossy<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> Utf8Lossy<W> {
    pub fn new(inner: W) -> Self {
        Utf8Lossy { inner, pending: Vec::new() }
    }

    fn emit(&mut self, mut data: &[u8]) -> io::Result<()> {
        loop {
            match std::str::from_utf8(data) {
                Ok(_) => return self.inner.write_all(data),
                Err(e) => {
                    let (valid, rest) = data.split_at(e.valid_up_to());
                    self.inner.write_all(valid)?;
                    match e.error_len() {
                        Some(bad) => {
                            self.inner.write_all("\u{FFFD}".as_bytes())?;
                            data = &rest[bad..];
                        }
                        None => {
                            self.pending.extend_from_slice(rest);
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

impl<W: Write> Write for Utf8Lossy<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_all(buf)?;
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.pending.is_empty() {
            return self.emit(buf);
        }
        let mut joined = std::mem::take(&mut self.pending);
        joined.extend_from_slice(buf);
        self.emit(&joined)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for Utf8Lossy<W> {
    fn drop(&mut self) {
        if !self.pending.is_empty() {
            let _ = self.inner.write_all("\u{FFFD}".as_bytes());
        }
    }
}

/// Rewrites a lone `\r` as `\n`, leaving `\r\n` untouched. Paired with
/// [`Delimiter::AnyNewline`](crate::tail::Delimiter): each progress-bar update
/// becomes a visible line instead of overwriting the previous one.
pub struct CrToLf<W: Write> {
    inner: W,
    /// The last write ended with `\r`; the next byte decides what it was.
    pending_cr: bool,
}

impl<W: Write> CrToLf<W> {
    pub fn new(inner: W) -> Self {
        CrToLf { inner, pending_cr: false }
    }
}

impl<W: Write> Write for CrToLf<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_all(buf)?;
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let Some(&first) = buf.first() else { return Ok(()) };
        if std::mem::take(&mut self.pending_cr) {
            self.inner.write_all(if first == b'\n' { b"\r" } else { b"\n" })?;
        }
        let mut start = 0;
        let mut from = 0;
        while let Some(p) = buf[from..].iter().position(|&b| b == b'\r') {
            let i = from + p;
            from = i + 1;
            match buf.get(i + 1) {
                Some(b'\n') => {}
                Some(_) => {
                    self.inner.write_all(&buf[start..i])?;
                    self.inner.write_all(b"\n")?;
                    start = i + 1;
                }
                None => {
                    self.pending_cr = true;
                    return self.inner.write_all(&buf[start..i]);
                }
            }
        }
        self.inner.write_all(&buf[start..])
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for CrToLf<W> {
    fn drop(&mut self) {
        if self.pending_cr {
            let _ = self.inner.write_all(b"\n");
        }
    }
}

/// Prints `==> name <==` headers when the output switches to another file.
pub struct Headers {
    enabled: bool,
    current: Option<usize>,
}

impl Headers {
    pub fn new(enabled: bool) -> Self {
        Headers { enabled, current: None }
    }

    /// Announces that data from operand `index` follows.
    pub fn switch_to(&mut self, index: usize, name: &str, out: &mut dyn Write) -> io::Result<()> {
        if !self.enabled || self.current == Some(index) {
            return Ok(());
        }
        let separator = if self.current.is_some() { "\n" } else { "" };
        self.current = Some(index);
        writeln!(out, "{separator}==> {name} <==")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lossy(parts: &[&[u8]]) -> String {
        let mut out = Vec::new();
        {
            let mut w = Utf8Lossy::new(&mut out);
            for p in parts {
                w.write_all(p).unwrap();
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn valid_utf8_passes_through() {
        assert_eq!(lossy(&["città".as_bytes()]), "città");
    }

    #[test]
    fn invalid_bytes_are_replaced() {
        // 0xE0 alone is the Windows-1252 encoding of "à": not UTF-8.
        assert_eq!(lossy(&[b"citt\xE0 ok"]), "citt\u{FFFD} ok");
    }

    #[test]
    fn split_sequences_are_rejoined() {
        let bytes = "à".as_bytes();
        assert_eq!(lossy(&[&bytes[..1], &bytes[1..]]), "à");
    }

    #[test]
    fn dangling_sequence_at_end() {
        assert_eq!(lossy(&[b"x\xC3"]), "x\u{FFFD}");
    }

    fn cr_to_lf(parts: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut w = CrToLf::new(&mut out);
            for p in parts {
                w.write_all(p).unwrap();
            }
        }
        out
    }

    #[test]
    fn lone_cr_becomes_lf() {
        assert_eq!(cr_to_lf(&[b"\rstep1\rstep2"]), b"\nstep1\nstep2");
        assert_eq!(cr_to_lf(&[b"a\r\nb\r\n"]), b"a\r\nb\r\n");
        assert_eq!(cr_to_lf(&[b"a\r\r\n"]), b"a\n\r\n");
    }

    #[test]
    fn cr_at_write_boundary() {
        assert_eq!(cr_to_lf(&[b"a\r", b"\nb"]), b"a\r\nb");
        assert_eq!(cr_to_lf(&[b"a\r", b"b"]), b"a\nb");
        assert_eq!(cr_to_lf(&[b"a\r", b"", b"\r", b"x"]), b"a\n\nx");
        assert_eq!(cr_to_lf(&[b"end\r"]), b"end\n");
    }

    #[test]
    fn headers() {
        let mut out = Vec::new();
        let mut h = Headers::new(true);
        h.switch_to(0, "a", &mut out).unwrap();
        h.switch_to(0, "a", &mut out).unwrap();
        h.switch_to(1, "b", &mut out).unwrap();
        assert_eq!(out, b"==> a <==\n\n==> b <==\n");
    }
}
