# tail-win: design notes

A native Windows port of GNU `tail`. This document records why the project is
built the way it is, so later changes keep the same trade-offs.

## 1. Language and source strategy

### Options considered

| Option | Verdict |
| --- | --- |
| Port GNU `tail.c` (C, MSVC) | Rejected. `tail.c` is about 2,500 lines welded to gnulib (`xstrtol`, `safe_read`, `xnanosleep`, `quotearg`, ...), POSIX `stat`/`fstat`/`ino_t`, signals and inotify. On Windows every one of those needs a shim, so we would maintain a fork of gnulib to keep one program alive. It is also GPLv3: copying it makes this project GPLv3. |
| C++ / Win32 from scratch | Viable, but no package manager in practice, manual memory safety, and testing infrastructure to build by hand. |
| Go | Easy, but a 2 MB+ runtime for a tool that is mostly syscalls, and weaker control over Win32 handle flags. |
| C# / .NET | Great Windows integration, but startup latency (JIT) is noticeable for a CLI that is often run in loops, unless AOT is configured. |
| **Rust (chosen)** | Native binary (~550 KB release), MSVC toolchain (it links with Visual Studio's `link.exe`), first-class Win32 bindings (`windows-sys`), `cargo test` built in, memory safety, and the core can be reused later as a library by a GUI. |

### Clean-room, behavior-compatible

The code is written from the GNU **documented behavior** (man page, `--help`,
observable output), not translated from `tail.c`. This keeps the license free
(MIT) and lets the structure fit Windows instead of POSIX. Compatibility is
verified by tests that pin GNU's observable behavior (output bytes, messages,
exit codes).

The [uutils/coreutils](https://github.com/uutils/coreutils) project (Rust, MIT)
also has a cross-platform `tail`. It is a useful reference for edge cases, but
it is part of a large multicall workspace; this project stays small and
Windows-first on purpose.

## 2. Architecture

```
main.rs        3 lines: calls app::run
app.rs         orchestration: operands -> initial output -> follow loop, exit code
cli.rs         clap definition + GNU obsolete syntax (-20, +5, -3cf) + validation
count.rs       NUM parsing with GNU suffixes (b, kB, K, KiB, M, ...), saturating
input.rs       FILE operands, "-" = stdin, wildcard expansion
tail.rs        the algorithms, generic over Read/Seek/Write (no OS knowledge)
follow.rs      --follow polling loop: appends, truncation, rotation, --retry, --pid
output.rs      stdout (console-safe UTF-8) and "==> name <==" headers
sys.rs         every Win32 call; the ONLY module with `unsafe`
```

Dependencies point downward only: `tail.rs` and `count.rs` do not know about
files, consoles or Windows, so they are tested with in-memory cursors and can be
reused unchanged by a future GUI (see section 6).

## 3. Algorithms

* **Last N lines of a file**: seek to the end and scan backwards in 64 KiB
  blocks counting delimiters. Cost is proportional to the output, not the file
  (a 165 MB log answers in tens of milliseconds).
* **Last N bytes of a file**: one seek.
* **From line/byte N (`+N`)**: skip forward, then a plain copy.
* **Pipes and consoles**: cannot seek, so a `ChunkBuffer` keeps only the
  trailing chunks that can still contribute to the answer. Memory is bounded by
  the size of the requested tail plus one buffer. Tiny pipe reads are coalesced.
* A trailing terminator ends the last record instead of starting a new one.
* **Line terminators (deliberate deviation from GNU).** By default
  (`Delimiter::AnyNewline`) a record ends at `\n`, `\r\n` or a lone `\r`, and
  the output layer (`CrToLf`) prints a lone `\r` as `\n`. Reason: training
  logs, pip, curl and other tqdm-style tools redraw progress with a bare `\r`,
  so GNU tail sees hours of updates as one huge line and the console shows
  only the last frame. `\r\n` is a single terminator and is never rewritten,
  so CRLF files are byte-identical. `--raw-cr` restores GNU semantics
  (`Delimiter::Byte(b'\n')`, no rewriting); `-z` uses NUL and also disables
  the CR handling. A `\r` at the end of a read buffer is undecided until the
  next byte arrives: the forward scan and `CrToLf` carry it over, the
  backward scan passes the first byte of the block to its right, and the
  stream buffer under-counts (safe) instead of guessing.

The unit tests run every case through both the seekable and the streaming path
and assert they agree, which catches divergence between the two
implementations.

## 4. Windows-specific decisions

| Problem | Decision |
| --- | --- |
| Log rotation renames/deletes the file we follow | Every file is opened with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, so tail never blocks the writer or the rotator. Tested by `follow_name_survives_rotation`. |
| No inotify; `ReadDirectoryChangesW` is unreliable on SMB shares | `--follow` polls every `-s` seconds (default 1.0), like GNU without inotify. Simple, deterministic, cheap. |
| NTFS updates the size in the directory entry lazily | Growth is detected through the **open handle** (`GetFileInformationByHandle`), never through a path `stat`. |
| No inode numbers | File identity for `--follow=name` is `FILE_ID_INFO` (volume serial + 128-bit id), which is also correct on ReFS. |
| `Metadata::is_file()` is true for pipe handles | Seekability is decided by `GetFileType == FILE_TYPE_DISK`. |
| Rust's console stdout rejects invalid UTF-8 | When stdout is a console, output goes through `Utf8Lossy` (invalid bytes become U+FFFD, split sequences are rejoined). Redirected output is never altered. |
| `cmd.exe` and PowerShell do not expand `*.log` | `tail` expands wildcards itself (case-insensitive) unless a file with that literal name exists. |
| `--pid` has no `kill(pid, 0)` | `OpenProcess(SYNCHRONIZE)` + `WaitForSingleObject(h, 0)`; access denied counts as alive. |
| Reader closes the pipe (`tail x | Select -First 1`) | Treated as normal termination: silent, exit 0. |
| Error text | Windows messages are mapped to GNU wording ("No such file or directory") so scripts and users see familiar output. |

## 5. Compatibility status

Implemented: `-c`, `-n` (with `+`, `-`, suffixes), `-f`, `--follow=name|descriptor`,
`-F`, `--retry`, `--pid`, `-s`, `-q`/`--quiet`/`--silent`, `-v`, `-z`,
obsolete forms (`-20`, `+5`, `-3cf`), `--max-unchanged-stats` (accepted,
ignored: this port re-checks the name every interval anyway), headers, exit
codes, GNU-style diagnostics.

Extensions: lone `\r` handling (section 3) with `--raw-cr` to opt out;
wildcard expansion (section 4).

Known gaps / future work:

* Legacy code page logs (Windows-1252) show U+FFFD on the console for accented
  characters. A fallback decode with the console's ANSI code page could fix it.
* UTF-16 files (Windows PowerShell 5 `>` output) are treated as bytes, as GNU
  does. An opt-in `--encoding` would be a Windows-only extension.
* Event-driven follow (`ReadDirectoryChangesW`) to cut latency below the poll
  interval, if ever needed.

## 6. Path to a GUI / Microsoft Store app

The crate is already split into a library (`tail_win`) and a 3-line binary.
For a Store app the recommended route is:

1. Keep `tail.rs`, `count.rs`, `follow.rs` as the engine. Before adding a GUI,
   replace the `report()` calls in `follow.rs` with an events trait
   (`on_data`, `on_truncated`, `on_replaced`, ...) so the GUI can render them.
2. Build the UI either in Rust (Tauri, or `windows-rs` + WinUI) or in C#
   WinUI 3 calling the engine through a `cdylib` C ABI.
3. Package as MSIX for the Store. The CLI already ships this way:
   `packaging/msix/` holds a manifest template and `build-msix.ps1`. The
   package is full trust (`runFullTrust`) and declares a console app
   execution alias, so `tail` is on PATH after install. Partner Center
   rejects headless packages (`AppListEntry="none"`) unless the account holds
   the HeadlessAppBypass waiver, so the package keeps a Start menu entry.
   Launched from there (or from Explorer), tail is the only process on a
   console it did not inherit (`sys::sole_console_process`) with no
   arguments and keyboard input; instead of reading the keyboard like GNU,
   which would look like a hang, it prints the help and a hint to use a
   terminal, then waits for Enter. In a shell the console is shared, so
   `tail` without arguments still reads stdin. The manifest version is the
   Cargo version reduced to `major.minor.patch.0`: MSIX only accepts a numeric
   quad and the Store reserves the fourth field, so it cannot carry a
   prerelease tag. Betas therefore set `[package.metadata.msix] version` in
   `Cargo.toml`. Beta 1 reached the Store as `1.0.0.0` and Store versions can
   only grow, so beta N ships as `1.0.N.0`. Drop the override for the release,
   whose derived version must sort above the last beta.
