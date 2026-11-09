# Cadence

A small floating window for Windows that downloads songs as MP3 and videos as MP4. Paste a link or type a song title, pick what you want, and Cadence saves it to your Music or Videos folder.

## Download

1. Go to the [latest release](https://github.com/jesubohr/Cadence/releases/latest) and download `Cadence-<version>-win64-setup.exe`.
2. Run it. The installer needs no administrator rights. Under **Download these tools**, keep yt-dlp, FFmpeg and Deno selected. Clear a tool only if you already have it on your PATH at the version shown or newer, because an older version makes Cadence fail. The installer downloads the selected tools from their own projects and checks each file against a pinned SHA-256. This needs an internet connection and takes a minute.
3. Start Cadence from the Start menu.

To remove Cadence, use **Settings > Apps > Installed apps**. The uninstaller asks if it should also delete your settings, queue and downloaded tools. Your downloaded music and videos stay where they are.

Windows 10 or 11 (64-bit) is required. The installer does not contain the tools, because their licenses (GPL) ask for source code that this project cannot host. If a tool is missing, Cadence tells you which one. Run **Install tools** from the Start menu, then press **Check again**.

Cadence is not code-signed yet, so Windows SmartScreen may warn on first start. Choose **More info**, then **Run anyway**.

To check your download, compare its SHA-256 with the `.sha256` file on the release page:

```powershell
Get-FileHash .\Cadence-<version>-win64-setup.exe -Algorithm SHA256
```

### Without the installer

Download `Cadence-<version>-win64.zip` and unzip it anywhere. Run `Install-Tools.cmd` once, then run `Cadence.exe`.

## What it does

- Paste a link, or type a song title to search. A search returns up to 15 results.
- Select one result or many. For a single video, Cadence shows the video heights that exist.
- MP3 at 128, 192 (default), 256 or 320 kbps, with cover art and tags. MP4 up to the height you choose.
- Two downloads run at the same time. The rest wait in a queue that survives a restart.
- Closing the window hides it in the system tray and downloads continue. Quit from the tray menu.
- Cadence never overwrites a file. If `Name.mp3` exists, it writes `Name (1).mp3`.

## Quick start

1. Start Cadence.
2. Paste a link or type a song title in the box, then submit it.
3. Choose **Audio** (MP3) or **Video** (MP4) and a quality.
4. Select the results you want and press **Download**. Follow progress in the **Queue** tab.

Audio goes to your Windows Music folder and video to your Videos folder. Change both in the settings (the sliders icon). Settings and the queue are stored in `%LOCALAPPDATA%\Cadence`.

## Known limits

- Windows x64 only.
- For search results and playlists, the video choices are a maximum height. Cadence downloads the best format at or below it.
- A link to a playlist loads the first 50 entries.
- Window position and size are not remembered, and there is no notification when a download ends.
- Live streams and sites that need a login are not supported.
- If Cadence or the PC stops during a download, a `.cadence-part-*` folder can stay in your destination folder. You can delete it.
- The tray menu was not tested in the automated test session. Please report problems.

Cadence is a front end for [yt-dlp](https://github.com/yt-dlp/yt-dlp). Make sure you may download the content from the site you use.

## Problems and ideas

Open an [issue](https://github.com/jesubohr/Cadence/issues). Sites change often. If a download fails, the error line shown in Cadence helps a lot.

## For developers

Build, test and release instructions are in [docs/DEVELOPMENT.md](https://github.com/jesubohr/Cadence/blob/main/docs/DEVELOPMENT.md).

## License

Cadence is released under the [MIT License](LICENSE).

The installer and the zip contain only Cadence and its install script. They do not redistribute yt-dlp, FFmpeg, FFprobe or Deno. `Install-Tools.cmd` downloads them from the upstream projects (yt-dlp and FFmpeg are GPL-3.0 programs, Deno is MIT) and Cadence starts them as separate programs, unchanged. `NOTICE.txt` in the zip lists the license, download link and SHA-256 of each tool, and `RUST_DEPENDENCIES.txt` lists the libraries inside `Cadence.exe`. The exact versions are pinned in [scripts/deps.json](https://github.com/jesubohr/Cadence/blob/main/scripts/deps.json).
