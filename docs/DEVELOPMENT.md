# Development

Cadence is a native Rust app built with [GPUI](https://www.gpui.rs). It runs the official `yt-dlp` executable for all downloading. GPUI is pre-1.0 and its Windows support is young, so the version is pinned (`=0.2.2` in `Cargo.toml`).

## Requirements

- Windows 10 or 11, x64.
- Rust 1.90 or newer (tested with 1.93.1) with the MSVC toolchain. Install the Visual Studio Build Tools with the "Desktop development with C++" workload and a Windows SDK.
- Internet access for the first build and for the tool download.
- [Inno Setup 6](https://jrsoftware.org/isinfo.php) to build the installer: `winget install JRSoftware.InnoSetup`.

## Run from source

```powershell
.\scripts\setup-deps.ps1      # yt-dlp, FFmpeg, FFprobe, Deno into %LOCALAPPDATA%\Cadence\tools
cargo run --release
```

Cadence needs **yt-dlp**, **FFmpeg**, **FFprobe** and **Deno**. Deno runs the JavaScript challenges that YouTube requires. If a tool is missing, the window shows a banner with the names and a "Check again" button. Install the tools, then press the button. No restart is needed.

### Where Cadence looks for tools

In this order, per tool:

1. The folder in the `CADENCE_TOOLS_DIR` environment variable
2. `tools\` next to `Cadence.exe`
3. `%LOCALAPPDATA%\Cadence\tools`
4. `PATH`

FFprobe must sit in the same folder as FFmpeg.

### The setup script

`scripts/setup-deps.ps1` installs the exact versions pinned in `scripts/deps.json`. It checks the SHA-256 of every download before it uses the file and stops on a mismatch. It skips tools that are already installed at the pinned version.

| Option | Effect |
| --- | --- |
| `-OutDir <path>` | Install somewhere else |
| `-Tools <names>` | Install only these tools, for example `-Tools yt-dlp,deno` |
| `-Force` | Download again |
| `-UpdateManifest` | Pin the latest releases first. Hashes come from the checksum files that yt-dlp, Deno and gyan.dev publish |

yt-dlp breaks when sites change, so update the pin from time to time. gyan.dev removes old FFmpeg builds. When the pinned FFmpeg URL returns 404, run `-UpdateManifest`.

## Tests

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -- --ignored   # needs network: downloads one thumbnail
```

The tests cover URL and search handling, yt-dlp arguments, JSON and progress parsing, file names and collisions, work folders (same-title MP3 and MP4 jobs, cancel, unrelated files with the same name prefix, files that appear during a download, parallel publishes to one name), tool lookup, config and queue persistence, the scheduler (two at a time, cancel, retry, duplicates), pipe draining with large output, and killing a process tree. The scheduler tests use a fake runner, so they need no network.

The `CI` workflow runs the first three commands on every push to `main` and on every pull request.

## Package

```powershell
.\scripts\package.ps1
```

This builds the release executable and writes the package. The package has no tools in it. It has `Install-Tools.cmd` and `scripts\` (the setup script and `deps.json`), so the user downloads the verified tools on their own PC. It writes:

- `dist\Cadence\` with `Cadence.exe`, `Install-Tools.cmd`, `scripts\`, `README.md`, `LICENSE`, `NOTICE.txt` and `RUST_DEPENDENCIES.txt`
- `dist\Cadence-<version>-win64.zip`
- `dist\Cadence-<version>-win64.zip.sha256`
- `dist\Cadence-<version>-win64-setup.exe`, built from `scripts\installer.iss`, and its `.sha256` file

Use `-SkipInstaller` to build only the folder and the zip.

### The installer

`scripts\installer.iss` installs the package folder for the current user into `%LOCALAPPDATA%\Programs\Cadence`. It needs no administrator rights. It adds Start menu shortcuts (Cadence and Install tools), an optional desktop shortcut and an entry in Settings > Apps. After the files are copied, it runs `scripts\setup-deps.ps1 -Tools <names>` to download the tools into `%LOCALAPPDATA%\Cadence	ools`. The task page has one checkbox per tool with its pinned version, and only the selected tools are downloaded. Setup asks to close a running Cadence. The uninstaller removes the program files and asks whether to delete `%LOCALAPPDATA%\Cadence` (settings, queue, tools). A silent uninstall keeps that folder.

Test it without the wizard: `Cadence-<version>-win64-setup.exe /VERYSILENT /DIR=<folder>`, then run `unins000.exe /VERYSILENT` in that folder.

`dist\` is ignored by git.

## Release

The `Release` workflow builds the zip and the installer and publishes them on GitHub when a tag that starts with `v` is pushed.

1. Change `version` in `Cargo.toml` and run `cargo build` so `Cargo.lock` follows.
2. Commit and push to `main`. Wait for `CI` to pass.
3. Tag and push: `git tag v<version>` and `git push origin v<version>`.

The workflow stops when the tag does not match the version in `Cargo.toml`, or when `Cargo.lock` is out of date. It creates the release with generated notes and uploads the installer, the zip and their `.sha256` files. They are small because they have no tools. If the release already exists, the workflow replaces the files.

## Licenses of bundled tools

Cadence does not redistribute yt-dlp, FFmpeg, FFprobe or Deno. `Install-Tools.cmd` downloads them from the upstream projects on the user's PC. `scripts/package.ps1` writes `NOTICE.txt` from `scripts/deps.json`. Cadence's own code stays MIT, and it starts the tools as separate processes.

Why the tools are not in the zip: yt-dlp and the gyan.dev FFmpeg build are GPL-3.0. Shipping them means that this project must host their exact corresponding source. gyan.dev publishes only the FFmpeg commit of a build. The build has about 35 external libraries (x264, x265, libvpx and others), and their exact versions and build scripts are not published as a source package. Upstream does not offer a source package that matches the binary, so this project cannot host one. Do not add GPL binaries to the zip unless you can host the exact source for them with the release.

## Architecture

```
src/
  main.rs        window, tray and queue start-up
  ui/            GPUI views: main view, text input, widgets, theme, icons
  queue.rs       job model, scheduler (2 parallel), cancel, retry, persistence
  ytdlp.rs       yt-dlp arguments, JSON and progress parsing, download runner
  staging.rs     per-job work folders and the move that never replaces a file
  process.rs     child process runner: no shell, drained pipes, job object kill
  tools.rs       tool lookup and missing-tool report
  paths.rs       safe file names and collision-free paths
  config.rs      persisted settings         storage.rs  JSON files, atomic writes
  thumbs.rs      thumbnail download and cache
  tray.rs        tray icon and menu         win.rs      hide, restore, topmost, single instance
  icon.rs        generated app icon, shared with build.rs
```

**Threads.** The UI thread never blocks on I/O. Searches, thumbnails, tool checks and config saves run on gpui's background executor. Each download has its own worker thread. A small writer thread saves `queue.json`. The queue wakes the UI through a one-slot channel, so bursts of progress updates collapse into one redraw.

**Processes.** Arguments go to `CreateProcess` as an argument list. There is no shell, so titles and URLs cannot inject commands. Stdout and stderr are read on separate threads until the process ends, so a chatty child cannot fill a pipe and hang. Every child joins a Windows job object with kill-on-close. Cancel terminates the job, which ends yt-dlp, its helper process and FFmpeg together. If Cadence itself is killed, Windows ends the same tree.

**Progress.** yt-dlp prints structured lines (`--progress-template` with `%(progress)j`). Cadence maps the byte counts of each stream to one steady bar. States are Queued, Downloading, Converting (any post-processing step such as audio extraction or merging), Completed, Failed and Cancelled. Failures show the last `ERROR` line from yt-dlp. A cancelled or failed job deletes its own work folder and nothing else.

**File names.** The target path is chosen when a job starts, after sanitizing the title for Windows and checking the disk and the other running jobs.

**Work folders.** Each job runs yt-dlp in its own new folder, `.cadence-part-<process>-<number>`, inside the destination folder. All yt-dlp files go there: partial streams, thumbnails and the converted result. Two jobs with the same title, such as an MP3 and an MP4, never share a file. When yt-dlp succeeds, Cadence moves the result to the target path with `MoveFileExW` and no replace flag. If a file with that name exists, or appears while the job runs, Cadence writes `Name (1).mp3` and so on. It never overwrites a file. Then Cadence deletes the work folder. Success, failure and cancel all delete only that folder, and no file outside it is touched. The work folder is on the same volume as the result, so the move is a rename.

**Window.** gpui has no hide or always-on-top API, so `win.rs` calls `ShowWindow` and `SetWindowPos` with the native window handle. The close button hides the window when the tray icon exists. A second launch shows the running window and exits.

## Data

Stored in `%LOCALAPPDATA%\Cadence`. Set `CADENCE_DATA_DIR` to change the location.

| File | Content |
| --- | --- |
| `config.json` | Folders, last kind and quality, always on top |
| `queue.json` | The queue. A job that was running when the app stopped starts again on the next launch |
| `thumbs\` | Thumbnail cache. Files older than 14 days are deleted at start |
