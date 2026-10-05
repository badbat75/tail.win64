//! The core "print the end of a stream" algorithms.
//!
//! Everything here is generic over `Read`/`Seek`/`Write` and knows nothing
//! about files, consoles or Windows, so it can be tested with in-memory
//! cursors and reused by any front end (CLI today, a GUI later).

use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom, Write};

use crate::count::{Count, Origin, Unit};

/// Size of every read. 64 KiB matches the NTFS/SMB sweet spot for sequential I/O.
pub const BUF_SIZE: usize = 64 * 1024;

/// What ends a record ("line").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    /// `\n`, `\r\n` or a lone `\r`. The default: progress bars (tqdm, curl,
    /// pip, ...) redraw themselves with a bare `\r`, so every update counts
    /// as a line instead of the whole history collapsing into one.
    AnyNewline,
    /// Exactly this byte, as GNU tail does (`--raw-cr` for `\n`, `-z` for NUL).
    Byte(u8),
}

impl Delimiter {
    /// Could `b` be (part of) a record terminator?
    fn is_candidate(self, b: u8) -> bool {
        match self {
            Delimiter::AnyNewline => b == b'\n' || b == b'\r',
            Delimiter::Byte(d) => b == d,
        }
    }

    /// Does the record end at `data[i]`? `next` is the byte that follows
    /// `data`, if known: a `\r` followed by `\n` is not an end, the `\n` is.
    fn ends_record(self, data: &[u8], i: usize, next: Option<u8>) -> bool {
        match (self, data[i]) {
            (Delimiter::AnyNewline, b'\n') => true,
            (Delimiter::AnyNewline, b'\r') => data.get(i + 1).copied().or(next) != Some(b'\n'),
            (Delimiter::AnyNewline, _) => false,
            (Delimiter::Byte(d), b) => b == d,
        }
    }

    /// Length of the terminator that ends `data`, 0 if it ends mid-record.
    fn trailing_len(self, data: &[u8]) -> usize {
        match self {
            Delimiter::AnyNewline if data.ends_with(b"\r\n") => 2,
            Delimiter::AnyNewline => usize::from(matches!(data.last(), Some(b'\n' | b'\r'))),
            Delimiter::Byte(d) => usize::from(data.last() == Some(&d)),
        }
    }
}

/// Writes the selected part of a seekable input (a regular file).
///
/// Returns the offset just past the last byte consumed, which is where a
/// subsequent `--follow` must resume reading.
pub fn tail_seekable<F, W>(input: &mut F, count: Count, delim: Delimiter, out: &mut W) -> io::Result<u64>
where
    F: Read + Seek,
    W: Write + ?Sized,
{
    match (count.origin, count.unit) {
        (Origin::FromEnd, Unit::Bytes) => {
            let len = input.seek(SeekFrom::End(0))?;
            input.seek(SeekFrom::Start(len.saturating_sub(count.n)))?;
        }
        (Origin::FromEnd, Unit::Lines) => {
            let start = find_last_lines_offset(input, count.n, delim)?;
            input.seek(SeekFrom::Start(start))?;
        }
        (Origin::FromStart, Unit::Bytes) => {
            // Clamped: a position past EOF would look like truncation to --follow.
            let len = input.seek(SeekFrom::End(0))?;
            input.seek(SeekFrom::Start(count.n.saturating_sub(1).min(len)))?;
        }
        (Origin::FromStart, Unit::Lines) => {
            input.seek(SeekFrom::Start(0))?;
            skip_lines(input, count.n.saturating_sub(1), delim, out)?;
        }
    }
    io::copy(input, out)?;
    input.stream_position()
}

/// Writes the selected part of a non-seekable input (pipe, console).
///
/// Memory use is bounded by the size of the requested tail plus one buffer.
pub fn tail_stream<R, W>(input: &mut R, count: Count, delim: Delimiter, out: &mut W) -> io::Result<()>
where
    R: Read + ?Sized,
    W: Write + ?Sized,
{
    match (count.origin, count.unit) {
        (Origin::FromStart, Unit::Bytes) => {
            io::copy(&mut input.take(count.n.saturating_sub(1)), &mut io::sink())?;
        }
        (Origin::FromStart, Unit::Lines) => {
            skip_lines(input, count.n.saturating_sub(1), delim, out)?;
        }
        (Origin::FromEnd, _) if count.n == 0 => return Ok(()),
        (Origin::FromEnd, unit) => {
            let buffer = ChunkBuffer::fill(input, unit, count.n, delim)?;
            return buffer.write_tail(unit, count.n, delim, out);
        }
    }
    io::copy(input, out)?;
    Ok(())
}

/// Consumes `n` delimiter-terminated records. Any bytes read past the last
/// skipped record are written to `out`, leaving `input` ready for a plain copy.
fn skip_lines<R, W>(input: &mut R, mut n: u64, delim: Delimiter, out: &mut W) -> io::Result<()>
where
    R: Read + ?Sized,
    W: Write + ?Sized,
{
    if n == 0 {
        return Ok(());
    }
    let mut buf = vec![0u8; BUF_SIZE];
    // A `\r` that ended the previous read: whether it ends a record depends
    // on the first byte of this one.
    let mut pending_cr = false;
    loop {
        let len = read_some(input, &mut buf)?;
        if len == 0 {
            return Ok(());
        }
        let data = &buf[..len];
        if std::mem::take(&mut pending_cr) && data[0] != b'\n' {
            n -= 1;
            if n == 0 {
                return out.write_all(data);
            }
        }
        let mut from = 0;
        while let Some(p) = data[from..].iter().position(|&b| delim.is_candidate(b)) {
            let i = from + p;
            from = i + 1;
            if i + 1 == len && delim == Delimiter::AnyNewline && data[i] == b'\r' {
                pending_cr = true;
            } else if delim.ends_record(data, i, None) {
                n -= 1;
                if n == 0 {
                    return out.write_all(&data[from..]);
                }
            }
        }
    }
}

/// Finds the offset where the last `n` records of a seekable input begin by
/// scanning backwards one block at a time.
fn find_last_lines_offset<F: Read + Seek>(input: &mut F, n: u64, delim: Delimiter) -> io::Result<u64> {
    let len = input.seek(SeekFrom::End(0))?;
    if n == 0 {
        return Ok(len);
    }
    let mut remaining = n;
    let mut end = len;
    let mut buf = vec![0u8; BUF_SIZE];
    // First byte of the block to the right of the one being scanned.
    let mut next: Option<u8> = None;
    let mut is_last_block = true;
    while end > 0 {
        let start = end.saturating_sub(BUF_SIZE as u64);
        let block = &mut buf[..(end - start) as usize];
        input.seek(SeekFrom::Start(start))?;
        input.read_exact(block)?;
        let mut scan: &[u8] = block;
        if is_last_block {
            (scan, next) = strip_trailing(scan, delim);
            is_last_block = false;
        }
        if let Some(i) = scan_back(scan, next, &mut remaining, delim) {
            return Ok(start + i as u64 + 1);
        }
        next = block.first().copied();
        end = start;
    }
    Ok(0)
}

/// Offset in `data` where its last `n` records start.
fn last_lines_offset(data: &[u8], n: u64, delim: Delimiter) -> usize {
    let mut remaining = n;
    let (scan, next) = strip_trailing(data, delim);
    match scan_back(scan, next, &mut remaining, delim) {
        Some(i) => i + 1,
        None => 0,
    }
}

/// A trailing terminator ends the last record, it does not start a new one.
/// Returns the data without it, plus the first byte removed (the context
/// needed to classify a `\r` right before it).
fn strip_trailing(data: &[u8], delim: Delimiter) -> (&[u8], Option<u8>) {
    let (rest, terminator) = data.split_at(data.len() - delim.trailing_len(data));
    (rest, terminator.first().copied())
}

/// Walks `data` backwards decrementing `remaining` at every record end.
/// Returns the index of the terminator that brings it to zero. `next` is the
/// byte following `data`, if any. `remaining` must be > 0.
fn scan_back(data: &[u8], next: Option<u8>, remaining: &mut u64, delim: Delimiter) -> Option<usize> {
    let mut end = data.len();
    while let Some(i) = data[..end].iter().rposition(|&b| delim.is_candidate(b)) {
        if delim.ends_record(data, i, next) {
            *remaining -= 1;
            if *remaining == 0 {
                return Some(i);
            }
        }
        end = i;
    }
    None
}

/// Counts record ends in `data`, never over-counting: a `\r` at the very end
/// is assumed to be the first half of a `\r\n`.
fn count_records(data: &[u8], delim: Delimiter) -> u64 {
    let ends = (0..data.len()).filter(|&i| delim.is_candidate(data[i]) && delim.ends_record(data, i, Some(b'\n')));
    ends.count() as u64
}

/// Retains only the trailing chunks of a stream that can still contribute to
/// the requested tail.
struct ChunkBuffer {
    chunks: VecDeque<Chunk>,
    total_bytes: u64,
    total_delims: u64,
}

struct Chunk {
    data: Vec<u8>,
    delims: u64,
}

impl ChunkBuffer {
    fn fill<R: Read + ?Sized>(input: &mut R, unit: Unit, n: u64, delim: Delimiter) -> io::Result<Self> {
        let mut this = ChunkBuffer { chunks: VecDeque::new(), total_bytes: 0, total_delims: 0 };
        let mut buf = vec![0u8; BUF_SIZE];
        loop {
            let len = read_some(input, &mut buf)?;
            if len == 0 {
                return Ok(this);
            }
            this.push(&buf[..len], delim);
            this.trim(unit, n);
        }
    }

    fn push(&mut self, data: &[u8], delim: Delimiter) {
        // Under-counting is safe (we just keep a chunk longer), over-counting is not.
        let delims = count_records(data, delim);
        self.total_bytes += data.len() as u64;
        self.total_delims += delims;
        // Pipes often deliver tiny reads: coalesce them to keep the deque short.
        if let Some(last) = self.chunks.back_mut() {
            if last.data.len() + data.len() <= BUF_SIZE {
                last.data.extend_from_slice(data);
                last.delims += delims;
                return;
            }
        }
        self.chunks.push_back(Chunk { data: data.to_vec(), delims });
    }

    /// Drops the oldest chunk while the newer ones alone are enough.
    fn trim(&mut self, unit: Unit, n: u64) {
        while let Some(front) = self.chunks.front() {
            let enough = match unit {
                Unit::Bytes => self.total_bytes - front.data.len() as u64 >= n,
                // One extra delimiter: a trailing one does not start a record.
                Unit::Lines => self.total_delims - front.delims > n,
            };
            if !enough {
                return;
            }
            self.total_bytes -= front.data.len() as u64;
            self.total_delims -= front.delims;
            self.chunks.pop_front();
        }
    }

    fn write_tail<W: Write + ?Sized>(self, unit: Unit, n: u64, delim: Delimiter, out: &mut W) -> io::Result<()> {
        let data: Vec<u8> = self.chunks.into_iter().flat_map(|c| c.data).collect();
        let start = match unit {
            Unit::Bytes => data.len() - n.min(data.len() as u64) as usize,
            Unit::Lines => last_lines_offset(&data, n, delim),
        };
        out.write_all(&data[start..])
    }
}

/// `Read::read` that retries on `Interrupted`.
pub fn read_some<R: Read + ?Sized>(input: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match input.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const LF: Delimiter = Delimiter::Byte(b'\n');
    const ANY: Delimiter = Delimiter::AnyNewline;

    fn count(unit: Unit, origin: Origin, n: u64) -> Count {
        Count { unit, origin, n }
    }

    /// Runs the seekable, the streaming and the one-byte-at-a-time streaming
    /// paths and checks they all agree.
    fn run_with(data: &[u8], c: Count, delim: Delimiter) -> Vec<u8> {
        let mut seek_out = Vec::new();
        let end = tail_seekable(&mut Cursor::new(data), c, delim, &mut seek_out).unwrap();
        assert_eq!(end, data.len() as u64, "seekable path must stop at EOF");
        let mut stream_out = Vec::new();
        tail_stream(&mut Cursor::new(data), c, delim, &mut stream_out).unwrap();
        assert_eq!(seek_out, stream_out, "seekable and stream paths disagree for {c:?} {delim:?}");
        let mut trickle_out = Vec::new();
        tail_stream(&mut Trickle(data), c, delim, &mut trickle_out).unwrap();
        assert_eq!(seek_out, trickle_out, "byte-by-byte stream disagrees for {c:?} {delim:?}");
        seek_out
    }

    /// For input without lone `\r`, both delimiter modes must give the same result.
    fn run(data: &[u8], c: Count) -> Vec<u8> {
        let lf = run_with(data, c, LF);
        assert_eq!(lf, run_with(data, c, ANY), "AnyNewline differs from LF for {c:?}");
        lf
    }

    fn last_lines(data: &[u8], n: u64) -> Vec<u8> {
        run(data, count(Unit::Lines, Origin::FromEnd, n))
    }

    fn last_any(data: &[u8], n: u64) -> Vec<u8> {
        run_with(data, count(Unit::Lines, Origin::FromEnd, n), ANY)
    }

    fn from_any(data: &[u8], n: u64) -> Vec<u8> {
        run_with(data, count(Unit::Lines, Origin::FromStart, n), ANY)
    }

    #[test]
    fn lone_cr_ends_a_line() {
        // A tqdm-style progress bar: one physical line, many updates.
        assert_eq!(last_any(b"start\n\rstep1\rstep2\rstep3", 2), b"step2\rstep3");
        assert_eq!(last_any(b"a\r\nb\rc\r\n", 2), b"b\rc\r\n");
        assert_eq!(last_any(b"a\rb\r", 1), b"b\r");
        assert_eq!(last_any(b"a\r\r\n", 1), b"\r\n");
        assert_eq!(last_any(b"\r\r\r", 2), b"\r\r");
        assert_eq!(from_any(b"a\r\nb\rc", 2), b"b\rc");
        assert_eq!(from_any(b"a\r\nb\rc", 3), b"c");
        assert_eq!(from_any(b"a\rb\r\nc", 3), b"c");
    }

    #[test]
    fn raw_cr_keeps_gnu_behavior() {
        let c = count(Unit::Lines, Origin::FromEnd, 1);
        assert_eq!(run_with(b"x\ny\rz", c, LF), b"y\rz");
        assert_eq!(run_with(b"x\ny\rz", c, ANY), b"z");
    }

    #[test]
    fn crlf_split_across_blocks() {
        // "\r" is the last byte of one backwards-scan block and "\n" the first
        // byte of the next: it must still count as a single terminator.
        let mut data = b"x\r\n".to_vec();
        data.extend(std::iter::repeat_n(b'y', BUF_SIZE - 1));
        assert_eq!(data.len() - BUF_SIZE, 2, "the \\n must open the last block");
        assert_eq!(last_any(&data, 2), data);
        assert_eq!(last_any(&data, 1), &data[3..]);
    }

    #[test]
    fn last_lines_basic() {
        assert_eq!(last_lines(b"a\nb\nc\n", 2), b"b\nc\n");
        assert_eq!(last_lines(b"a\nb\nc", 2), b"b\nc");
        assert_eq!(last_lines(b"a\nb\nc\n", 10), b"a\nb\nc\n");
        assert_eq!(last_lines(b"a\nb\nc\n", 0), b"");
        assert_eq!(last_lines(b"", 3), b"");
        assert_eq!(last_lines(b"\n\n\n", 2), b"\n\n");
        assert_eq!(last_lines(b"no newline", 1), b"no newline");
    }

    #[test]
    fn crlf_is_preserved() {
        assert_eq!(last_lines(b"a\r\nb\r\nc\r\n", 2), b"b\r\nc\r\n");
    }

    #[test]
    fn from_start() {
        let data = b"1\n2\n3\n4\n";
        assert_eq!(run(data, count(Unit::Lines, Origin::FromStart, 3)), b"3\n4\n");
        assert_eq!(run(data, count(Unit::Lines, Origin::FromStart, 0)), data);
        assert_eq!(run(data, count(Unit::Lines, Origin::FromStart, 1)), data);
        assert_eq!(run(data, count(Unit::Lines, Origin::FromStart, 99)), b"");
        assert_eq!(run(data, count(Unit::Bytes, Origin::FromStart, 3)), b"2\n3\n4\n");
        assert_eq!(run(data, count(Unit::Bytes, Origin::FromStart, 99)), b"");
        let crlf = b"1\r\n2\r\n3\r\n";
        assert_eq!(run(crlf, count(Unit::Lines, Origin::FromStart, 3)), b"3\r\n");
    }

    #[test]
    fn last_bytes() {
        let data = b"hello world";
        assert_eq!(run(data, count(Unit::Bytes, Origin::FromEnd, 5)), b"world");
        assert_eq!(run(data, count(Unit::Bytes, Origin::FromEnd, 0)), b"");
        assert_eq!(run(data, count(Unit::Bytes, Origin::FromEnd, 100)), data);
    }

    #[test]
    fn zero_terminated() {
        let data = b"a\0b\rc\0";
        let c = count(Unit::Lines, Origin::FromEnd, 1);
        assert_eq!(run_with(data, c, Delimiter::Byte(0)), b"b\rc\0");
    }

    #[test]
    fn spans_many_blocks() {
        // Lines long enough that the backwards scan and the chunk buffer both
        // have to cross several BUF_SIZE boundaries.
        let line = vec![b'x'; BUF_SIZE / 3];
        let mut data = Vec::new();
        for i in 0..20u8 {
            data.extend_from_slice(&line);
            data.push(b'0' + i % 10);
            data.push(b'\n');
        }
        let expected = data[data.len() - 7 * (line.len() + 2)..].to_vec();
        assert_eq!(last_lines(&data, 7), expected);
        let bytes = run(&data, count(Unit::Bytes, Origin::FromEnd, BUF_SIZE as u64 + 17));
        assert_eq!(bytes, data[data.len() - BUF_SIZE - 17..]);
    }

    /// A reader that hands out one byte per call, like a slow pipe.
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.split_first() {
                Some((&b, rest)) if !buf.is_empty() => {
                    buf[0] = b;
                    self.0 = rest;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }
}
