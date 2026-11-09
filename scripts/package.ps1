<#
.SYNOPSIS
  Builds a release folder, a zip and a setup program in dist.

.DESCRIPTION
  Builds the release executable and writes the package: Cadence.exe, the tool
  install script with its pinned manifest, and the license notices. The package
  does not contain yt-dlp, FFmpeg, FFprobe or Deno. Install-Tools.cmd downloads
  them from the upstream projects and checks the pinned SHA-256 of each file.
  The setup program (Inno Setup) installs the same files and downloads the tools.
  A SHA-256 file is written next to the zip and next to the setup program.

.PARAMETER SkipBuild
  Reuse an existing target\release\cadence.exe.

.PARAMETER SkipInstaller
  Do not build the setup program. Without this switch, Inno Setup 6 must be installed.
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipInstaller
)

$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'Cadence'

Push-Location $root
try {
    if (-not $SkipBuild) {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    }
    $exe = Join-Path $root 'target\release\cadence.exe'
    if (-not (Test-Path -LiteralPath $exe)) { throw "Missing $exe. Run without -SkipBuild." }

    $version = [regex]::Match((Get-Content -Raw -LiteralPath 'Cargo.toml'), '(?m)^version = "([^"]+)"').Groups[1].Value

    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'scripts') | Out-Null
    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'Cadence.exe')
    Copy-Item -LiteralPath 'README.md', 'LICENSE' -Destination $stage
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'setup-deps.ps1'), (Join-Path $PSScriptRoot 'deps.json') -Destination (Join-Path $stage 'scripts')

    $install = @(
        '@echo off',
        'powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\setup-deps.ps1"',
        'if errorlevel 1 (echo. & echo The tool install failed. Read the message above.) else (echo. & echo Done. Start Cadence.exe.)',
        'pause'
    )
    Set-Content -LiteralPath (Join-Path $stage 'Install-Tools.cmd') -Value $install -Encoding ASCII

    $manifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'deps.json') | ConvertFrom-Json
    $tools = $manifest.tools
    $notes = [ordered]@{
        'yt-dlp' = 'GPL-3.0-or-later (the standalone executable bundles GPL code; the yt-dlp code itself is the Unlicense). Source: https://github.com/yt-dlp/yt-dlp/tree/{0}'
        'deno'   = 'MIT. Source: https://github.com/denoland/deno/tree/v{0}'
        'ffmpeg' = 'GPL-3.0 build by gyan.dev. Source: https://github.com/FFmpeg/FFmpeg and the libraries listed at https://www.gyan.dev/ffmpeg/builds/'
    }
    $lines = @(
        'Cadence is MIT licensed. See LICENSE.',
        'The Rust libraries inside Cadence.exe are listed in RUST_DEPENDENCIES.txt.',
        '',
        'This package does not contain yt-dlp, FFmpeg, FFprobe or Deno.',
        'Install-Tools.cmd downloads them on your PC from the upstream projects below.',
        'The download is checked against the SHA-256 pinned in scripts\deps.json.',
        'Cadence starts these programs as separate processes and does not change them.',
        'Each program stays under its own license and is distributed by its own project.',
        ''
    )
    foreach ($name in $notes.Keys) {
        $lines += "$name $($tools.$name.version)"
        $lines += "  license:  $($notes[$name] -f $tools.$name.version)"
        $lines += "  download: $($tools.$name.url)"
        $lines += "  sha256:   $($tools.$name.sha256)"
    }
    Set-Content -LiteralPath (Join-Path $stage 'NOTICE.txt') -Value $lines -Encoding ASCII

    $crates = cargo tree --locked --target x86_64-pc-windows-msvc -e normal --prefix none -f '{p}|{l}|{r}'
    if ($LASTEXITCODE -ne 0) { throw 'cargo tree failed' }
    $crates = $crates | ForEach-Object { $_ -replace ' \((\*|proc-macro)\)', '' -replace '\|', ' | ' } |
        Where-Object { $_ -notmatch '^cadence ' } | Sort-Object -Unique
    Set-Content -LiteralPath (Join-Path $stage 'RUST_DEPENDENCIES.txt') -Value (@('name version | license | repository', '') + $crates) -Encoding ASCII

    $zip = Join-Path $dist "Cadence-$version-win64.zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    # tar writes zip entries with forward slashes. Windows PowerShell 5.1 writes backslashes.
    & (Join-Path $env:SystemRoot 'System32\tar.exe') -a -c -f $zip -C $dist 'Cadence'
    if ($LASTEXITCODE -ne 0) { throw 'tar failed' }

    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
    # LF only, so sha256sum -c works on Linux and macOS as well.
    [IO.File]::WriteAllText("$zip.sha256", "$hash  $(Split-Path -Leaf $zip)`n")

    $installer = $null
    if (-not $SkipInstaller) {
        $iscc = @(
            (Get-Command iscc -ErrorAction SilentlyContinue).Source,
            (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
            (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
            (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
        ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
        if (-not $iscc) { throw 'Inno Setup 6 not found. Run "winget install JRSoftware.InnoSetup" or pass -SkipInstaller.' }

        # build.rs writes the icon that is embedded in Cadence.exe.
        $icon = Get-ChildItem -Path (Join-Path $root 'target\release\build\cadence-*\out\cadence.ico') |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (-not $icon) { throw 'Missing cadence.ico. Run without -SkipBuild.' }

        & $iscc /Qp "/DAppVersion=$version" "/DStageDir=$stage" "/DOutputDir=$dist" "/DIconFile=$($icon.FullName)" `
            "/DYtdlpVersion=$($tools.'yt-dlp'.version)" "/DFfmpegVersion=$($tools.ffmpeg.version)" "/DDenoVersion=$($tools.deno.version)" `
            (Join-Path $PSScriptRoot 'installer.iss')
        if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }

        $installer = Join-Path $dist "Cadence-$version-win64-setup.exe"
        $installerHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
        [IO.File]::WriteAllText("$installer.sha256", "$installerHash  $(Split-Path -Leaf $installer)`n")
    }

    Write-Host "Package folder: $stage"
    Write-Host "Package zip:    $zip"
    Write-Host "SHA-256:        $hash"
    if ($installer) {
        Write-Host "Installer:      $installer"
        Write-Host "SHA-256:        $installerHash"
    }
} finally {
    Pop-Location
}
