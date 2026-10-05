# tail-win

A native Windows port of GNU `tail`: same options, same output, no Cygwin/MSYS
runtime. Single `tail.exe`, about 550 KB.

```powershell
tail app.log                    # last 10 lines
tail -n 50 app.log              # last 50 lines
tail -n +100 app.log            # from line 100 to the end
tail -c 1K app.log              # last 1024 bytes
tail -f app.log                 # follow appended data
tail -F app.log                 # follow by name: survives log rotation
tail -f --pid 1234 app.log      # stop following when process 1234 exits
tail -q *.log                   # wildcards work in cmd and PowerShell too
Get-Content x | tail -n 3       # standard input
tail --raw-cr -n 5 train.log    # GNU handling of carriage returns (see below)
```

**One deliberate difference from GNU:** a carriage return not followed by a
newline also ends a line and is printed as a newline. Progress bars (tqdm in
training logs, pip, curl) redraw with a bare `\r`; GNU tail treats the whole
history as one line, here every update is a line, so `tail -n 50 train.log`
shows the last 50 steps. CRLF files are not altered. `--raw-cr` restores the
GNU behavior.

Run `tail --help` for the full option list.

## Build

Requires the Rust MSVC toolchain (`rustup`, with Visual Studio Build Tools).

```powershell
cargo build --release           # target\release\tail.exe
cargo test                      # unit + end-to-end tests
```

## MSIX package

```powershell
.\packaging\msix\build-msix.ps1                  # target\msix\tail-win_1.0.0-beta.1_x64.msix (unsigned)
.\packaging\msix\build-msix.ps1 -Arch arm64      # needs: rustup target add aarch64-pc-windows-msvc
.\packaging\msix\build-msix.ps1 -CertificateThumbprint <sha1>   # signed, for sideloading
.\packaging\msix\build-msix.ps1 -SkipCertification              # no App Certification Kit run
```

After packing, the script runs the Windows App Certification Kit (the checks
Partner Center applies) and writes `target\msix\wack-report.xml`; it fails if
the kit reports FAIL. `appcert.exe` needs elevation, so expect a UAC prompt.

Requires the Windows SDK (`makeappx.exe`, `signtool.exe`). The package
registers an app execution alias, so `tail` is on PATH after install. Its
Start menu entry (required by the Store) opens a console with the help.
The MSIX version is derived from `Cargo.toml` (`1.0.0-beta.1` becomes
`1.0.0.0` inside the package, the Store reserves the fourth field; the file
name keeps the full version).
To sideload, the certificate subject must equal `-Publisher` and the
certificate must be trusted on the machine. The package identity defaults to
the Store reservation (`BadBat75.tailforWindows`, publisher
`CN=932406D5-4DDE-483C-9D6C-7517FB42206B`), so the unsigned package can be
submitted to Partner Center as is. The Store listing texts and screenshots
are versioned in [packaging/store](packaging/store/listing.md).

Design rationale and Windows-specific decisions: [docs/DESIGN.md](docs/DESIGN.md).
Privacy: [PRIVACY.md](PRIVACY.md) (tail collects no data).
