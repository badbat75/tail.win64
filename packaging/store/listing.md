# Microsoft Store listing

Source of truth for the Partner Center submission of the reserved product
"tail for Windows". Edit here first, then paste into Partner Center. Images
live next to this file: `screenshots/` (PNG, at least 1366x768) and `logos/`
(Store logos: `tile-300x300.png` is the 1:1 app tile icon, `tile-150x150.png`
and `tile-71x71.png` the other tile sizes; same drawing as the MSIX assets).

## Properties

* **Category:** Developer tools
* **Privacy policy URL:** https://github.com/badbat75/tail.win64/blob/main/PRIVACY.md
* **Website:** https://github.com/badbat75/tail.win64
* **Support contact:** https://github.com/badbat75/tail.win64/issues

## Pricing and availability

* **Price:** Free
* **Markets:** all

## Store listing (English)

### Short description

GNU tail for Windows: show the last lines of files and follow logs live. Native, single exe, no Cygwin.

### Description

tail for Windows is a native port of GNU tail. It shows the end of text files and follows log files as they grow, with the same options and output as on Linux.

- tail app.log: last 10 lines
- tail -n 50 app.log, tail -c 1K app.log: last lines or bytes
- tail -n +100 app.log: from line 100 to the end
- tail -f app.log: follow appended data
- tail -F app.log: follow by name, survives log rotation
- tail -f --pid 1234 app.log: stop when a process exits
- Wildcards work in cmd and PowerShell; standard input is supported

Built for Windows: it never locks the files it reads, so writers and log rotation keep working. Progress bars redrawn with a carriage return (tqdm, pip, curl) are shown as separate lines; --raw-cr restores the GNU behavior.

After installation, the tail command is available in any terminal. Free and open source (MIT): https://github.com/badbat75/tail.win64

This is a beta release.

### What's new in this version

The Start menu window is now titled "tail for Windows" instead of showing the path of tail.exe.

### Product features

- Same options and output as GNU tail
- Follow files live with -f and -F (log rotation aware)
- Never locks files: writers and rotation keep working
- Wildcards in cmd and PowerShell
- Handles progress bars written with carriage returns
- Native single exe, no Cygwin or MSYS runtime

### Search terms

tail, log viewer, follow log, command line, GNU coreutils, tail -f, developer tools

### Copyright and trademark info

© 2026 Emiliano De Simoni

## Submission options

### Restricted capability justification (runFullTrust)

tail is a command-line tool (a Win32 console executable) started from the terminal through an app execution alias. It must read arbitrary files and standard input that the user names on the command line, and monitor them for changes, which requires full trust. It has no graphical UI (its Start menu entry only opens a console showing usage help), makes no network connections and collects no data.

## Age rating

IARC questionnaire: no violence, no user interaction, no data sharing, no
purchases. Expected result: 3+ / Everyone.
