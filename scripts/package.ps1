<#
.SYNOPSIS
  Builds a release folder and zip: dist\Cadence and dist\Cadence-<version>-win64.zip.

.DESCRIPTION
  Builds the release executable, downloads the pinned and verified tools into
  the package (see setup-deps.ps1), and writes the license notices. The result
  runs without any other setup. A SHA-256 file is written next to the zip.

.PARAMETER SkipBuild
  Reuse an existing target\release\cadence.exe.
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild
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
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'tools') | Out-Null
    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'Cadence.exe')
    Copy-Item -LiteralPath 'README.md', 'LICENSE' -Destination $stage

    & (Join-Path $PSScriptRoot 'setup-deps.ps1') -OutDir (Join-Path $stage 'tools')

    $manifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'deps.json') | ConvertFrom-Json
    $tools = $manifest.tools
    $notes = [ordered]@{
        'yt-dlp' = @{
            License = 'GPL-3.0-or-later. The standalone executable bundles GPL code. The yt-dlp code itself is the Unlicense.'
            Source  = "https://github.com/yt-dlp/yt-dlp/tree/$($tools.'yt-dlp'.version)"
        }
        'deno'   = @{
            License = 'MIT'
            Source  = "https://github.com/denoland/deno/tree/v$($tools.deno.version)"
        }
        'ffmpeg' = @{
            License = 'GPL-3.0. Build by gyan.dev with the GPL parts enabled.'
            Source  = "https://ffmpeg.org/releases/ffmpeg-$($tools.ffmpeg.version).tar.xz and https://www.gyan.dev/ffmpeg/builds/"
        }
    }
    $lines = @(
        'Cadence is MIT licensed. See LICENSE.',
        'The Rust libraries inside Cadence.exe are listed in RUST_DEPENDENCIES.txt.',
        '',
        'Cadence bundles these programs unchanged and runs them as separate processes.',
        'The GPL-3.0 text is in tools\ffmpeg-LICENSE.txt.',
        ''
    )
    foreach ($name in $notes.Keys) {
        $lines += "$name $($tools.$name.version)"
        $lines += "  license:  $($notes[$name].License)"
        $lines += "  download: $($tools.$name.url)"
        $lines += "  source:   $($notes[$name].Source)"
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

    Write-Host "Package folder: $stage"
    Write-Host "Package zip:    $zip"
    Write-Host "SHA-256:        $hash"
} finally {
    Pop-Location
}
