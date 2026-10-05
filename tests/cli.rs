//! End-to-end tests driving the real `tail` binary.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TAIL: &str = env!("CARGO_BIN_EXE_tail");

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tail-win-it-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tail(args: &[&str], dir: &Path) -> Output {
    Command::new(TAIL).args(args).current_dir(dir).stdin(Stdio::null()).output().unwrap()
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn last_ten_lines_by_default() {
    let s = Scratch::new("default");
    s.file("a.txt", numbered(15).as_bytes());
    let out = tail(&["a.txt"], &s.0);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        numbered(15).lines().skip(5).map(|l| format!("{l}\n")).collect::<String>()
    );
}

#[test]
fn headers_for_several_files() {
    let s = Scratch::new("headers");
    s.file("a.txt", b"1\n2\n");
    s.file("b.txt", b"3\n");
    let out = tail(&["-n", "1", "a.txt", "b.txt"], &s.0);
    assert_eq!(out.stdout, b"==> a.txt <==\n2\n\n==> b.txt <==\n3\n");
    let out = tail(&["-q", "-n", "1", "a.txt", "b.txt"], &s.0);
    assert_eq!(out.stdout, b"2\n3\n");
}

#[test]
fn wildcards_are_expanded() {
    let s = Scratch::new("glob");
    s.file("x1.log", b"one\n");
    s.file("x2.log", b"two\n");
    let out = tail(&["-q", "x*.log"], &s.0);
    assert_eq!(out.stdout, b"one\ntwo\n");
}

#[test]
fn missing_file_fails_but_others_print() {
    let s = Scratch::new("missing");
    s.file("a.txt", b"ok\n");
    let out = tail(&["-q", "nope.txt", "a.txt"], &s.0);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, b"ok\n");
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(err.trim_end(), "tail: cannot open 'nope.txt' for reading: No such file or directory");
}

#[test]
fn stdin_pipe_and_redirect() {
    let s = Scratch::new("stdin");
    let path = s.file("in.txt", numbered(30).as_bytes());

    let mut child = Command::new(TAIL).args(["-n", "2"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(numbered(30).as_bytes()).unwrap();
    assert_eq!(child.wait_with_output().unwrap().stdout, b"line 29\nline 30\n");

    let out = Command::new(TAIL).args(["-c", "8"]).stdin(File::open(&path).unwrap()).output().unwrap();
    assert_eq!(out.stdout, b"line 30\n");
}

#[test]
fn raw_bytes_pass_through_pipes() {
    let s = Scratch::new("bytes");
    s.file("bin.dat", b"caf\xE9\r\n\xFF\xFE\0end");
    let out = tail(&["-c", "+1", "bin.dat"], &s.0);
    assert_eq!(out.stdout, b"caf\xE9\r\n\xFF\xFE\0end");
}

#[test]
fn progress_bar_updates_are_lines() {
    let s = Scratch::new("cr");
    s.file("train.log", b"setup\r\n\rstep 1/3\rstep 2/3\rstep 3/3\n");
    assert_eq!(tail(&["-n", "2", "train.log"], &s.0).stdout, b"step 2/3\nstep 3/3\n");
    // GNU behavior: the whole progress bar is one line, printed verbatim.
    assert_eq!(tail(&["--raw-cr", "-n", "1", "train.log"], &s.0).stdout, b"\rstep 1/3\rstep 2/3\rstep 3/3\n");
    // CRLF files are not altered.
    s.file("win.log", b"a\r\nb\r\n");
    assert_eq!(tail(&["-n", "1", "win.log"], &s.0).stdout, b"b\r\n");
}

#[test]
fn obsolete_syntax() {
    let s = Scratch::new("obsolete");
    s.file("a.txt", numbered(5).as_bytes());
    assert_eq!(tail(&["-2", "a.txt"], &s.0).stdout, b"line 4\nline 5\n");
    assert_eq!(tail(&["+4", "a.txt"], &s.0).stdout, b"line 4\nline 5\n");
}

#[test]
fn usage_errors_exit_1() {
    let s = Scratch::new("usage");
    let out = tail(&["-n", "zz"], &s.0);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8(out.stderr).unwrap().contains("invalid number of lines: 'zz'"));
    assert!(tail(&["--help"], &s.0).status.success());
}

/// A running `tail -f` whose stdout is collected on a background thread.
struct Follower {
    child: Child,
    rx: mpsc::Receiver<u8>,
    seen: Vec<u8>,
}

impl Follower {
    fn spawn(args: &[&str], dir: &Path) -> Self {
        let mut child = Command::new(TAIL)
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 || buf[..n].iter().any(|&b| tx.send(b).is_err()) {
                    break;
                }
            }
        });
        Follower { child, rx, seen: Vec::new() }
    }

    /// Waits until the accumulated output ends with `expected`.
    fn expect(&mut self, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.seen.ends_with(expected.as_bytes()) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(b) => self.seen.push(b),
                Err(_) => panic!("timed out waiting for {expected:?}; got {:?}", String::from_utf8_lossy(&self.seen)),
            }
        }
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn append(path: &Path, data: &str) {
    OpenOptions::new().append(true).open(path).unwrap().write_all(data.as_bytes()).unwrap();
}

#[test]
fn follow_appends_and_truncation() {
    let s = Scratch::new("follow");
    let path = s.file("app.log", b"old\n");
    let mut f = Follower::spawn(&["-f", "-s", "0.05", "app.log"], &s.0);
    f.expect("old\n");
    append(&path, "new 1\n");
    f.expect("new 1\n");
    append(&path, "new 2\n");
    f.expect("new 2\n");
    fs::write(&path, "fresh\n").unwrap();
    f.expect("fresh\n");
}

#[test]
fn follow_prints_progress_updates_as_lines() {
    let s = Scratch::new("follow-cr");
    let path = s.file("train.log", b"start\n");
    let mut f = Follower::spawn(&["-f", "-s", "0.05", "train.log"], &s.0);
    f.expect("start\n");
    append(&path, "\rstep 1");
    f.expect("start\n\nstep 1");
    append(&path, "\rstep 2");
    f.expect("step 1\nstep 2");
}

#[test]
fn follow_name_survives_rotation() {
    let s = Scratch::new("rotate");
    let path = s.file("app.log", b"first\n");
    let mut f = Follower::spawn(&["-F", "-s", "0.05", "app.log"], &s.0);
    f.expect("first\n");
    // The rename must succeed while tail holds the file open (FILE_SHARE_DELETE).
    fs::rename(&path, s.0.join("app.log.1")).unwrap();
    fs::write(&path, "second\n").unwrap();
    f.expect("second\n");
    append(&path, "third\n");
    f.expect("third\n");
}

#[test]
fn follow_retry_waits_for_file() {
    let s = Scratch::new("retry");
    let mut f = Follower::spawn(&["-F", "-s", "0.05", "later.log"], &s.0);
    std::thread::sleep(Duration::from_millis(200));
    s.file("later.log", b"hello\n");
    f.expect("hello\n");
}

#[test]
fn follow_stops_when_pid_exits() {
    let s = Scratch::new("pid");
    s.file("a.log", b"x\n");
    // Any short-lived process will do: another tail blocked on its stdin.
    let mut writer = Command::new(TAIL).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
    let pid = writer.id().to_string();
    let mut follower = Command::new(TAIL)
        .args(["-f", "-s", "0.05", "--pid", &pid, "a.log"])
        .current_dir(&s.0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(writer.stdin.take());
    writer.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = follower.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "tail did not exit after --pid died");
        std::thread::sleep(Duration::from_millis(50));
    }
}
