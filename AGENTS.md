# AGENTS.md

Guidance for coding agents working on tail-win, a clean-room Rust port of GNU
`tail` for Windows. Read `docs/DESIGN.md` before structural changes.

## Commands

```powershell
cargo build --release
cargo test                               # unit tests + tests/cli.rs (drives tail.exe)
cargo clippy --all-targets -- -D warnings
cargo fmt                                # rustfmt.toml: max_width 120
```

All four must pass before a change is done.

```powershell
.\packaging\msix\build-msix.ps1          # MSIX in target\msix (needs the Windows SDK)
```

The script then runs the Windows App Certification Kit (UAC prompt; report in
`target\msix\wack-report.xml`). Pass `-SkipCertification` in non-interactive runs.

The MSIX version comes from `Cargo.toml` (`[package.metadata.msix] version`
while in beta, `1.0.N.0` for beta N, since beta 1 shipped as `1.0.0.0`; otherwise the crate version); never
hard-code it in `packaging/msix/AppxManifest.xml` (its `{{...}}` tokens are
filled by the script).
Likewise `build.rs` embeds the exe's VERSIONINFO resource from `Cargo.toml`
(via `winresource`, which needs `rc.exe` from the Windows SDK); it is skipped
when the target OS is not Windows.

## Rules

* **Clean-room.** Never copy code from GNU coreutils (`tail.c`, gnulib): it is
  GPLv3 and this crate is MIT. Match GNU *behavior* (output bytes, messages,
  exit codes); check the GNU manual or a real GNU tail, not its source.
* **Layering.** `tail.rs` and `count.rs` stay free of OS and file-system
  knowledge (generic `Read`/`Seek`/`Write`). All Win32 calls and every
  `unsafe` block live in `sys.rs`, each with a `// SAFETY:` comment, and with
  a non-Windows fallback so the crate still builds on Linux.
* **Open files only through `sys::open_shared`** (shares READ, WRITE, DELETE),
  so tail never blocks writers or log rotation.
* **Detect regular files with `sys::is_regular_file`**, not
  `Metadata::is_file()`, which is also true for pipe handles on Windows.
* **Follow mode reads sizes from the open handle**, never from a path stat
  (NTFS updates directory entries lazily).
* **Lone `\r` is a line end by default** (`Delimiter::AnyNewline` + the
  `CrToLf` output writer); `--raw-cr` is the GNU mode. This is the one
  intentional GNU deviation besides wildcard expansion: keep `\r\n` intact and
  keep the seekable, streaming and byte-by-byte paths in agreement (the
  `run_with` test helper checks all three).
* Diagnostics go through `report()` with GNU wording; I/O errors through
  `describe()`. Usage errors exit 1, like GNU.
* New behavior needs a test: algorithm cases in `tail.rs` (they run through
  both the seekable and the streaming path), end-to-end cases in `tests/cli.rs`.
