<#
.SYNOPSIS
  Installs yt-dlp, FFmpeg, FFprobe and Deno for Cadence.

.DESCRIPTION
  Downloads the exact versions pinned in deps.json, verifies each SHA-256
  checksum before use, and copies the executables into OutDir.
  Cadence finds that folder without extra configuration.

.PARAMETER OutDir
  Target folder. Default: %LOCALAPPDATA%\Cadence\tools

.PARAMETER Force
  Download again even when the pinned version is already installed.

.PARAMETER UpdateManifest
  Rewrites deps.json with the latest upstream releases first. The hashes come
  from the checksum files that the upstream projects publish.
#>
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $env:LOCALAPPDATA 'Cadence\tools'),
    [switch]$Force,
    [switch]$UpdateManifest
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Add-Type -AssemblyName System.IO.Compression.FileSystem

$ManifestPath = Join-Path $PSScriptRoot 'deps.json'
$Headers = @{ 'User-Agent' = 'cadence-setup' }

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Get-Text([string]$Url) {
    $content = (Invoke-WebRequest -Uri $Url -UseBasicParsing -Headers $Headers).Content
    if ($content -is [byte[]]) { $content = [Text.Encoding]::UTF8.GetString($content) }
    $content
}

function Get-HashFrom([string]$Text) {
    $match = [regex]::Match($Text, '\b[0-9a-fA-F]{64}\b')
    if (-not $match.Success) { throw "No SHA-256 value found in: $Text" }
    $match.Value.ToLowerInvariant()
}

function Save-Verified([string]$Url, [string]$Sha256, [string]$Destination) {
    Write-Host "  downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing -Headers $Headers
    $actual = Get-Sha256 $Destination
    if ($actual -ne $Sha256.ToLowerInvariant()) {
        Remove-Item -LiteralPath $Destination -Force
        throw "Checksum mismatch for $Url`n  expected $Sha256`n  actual   $actual"
    }
    Write-Host "  sha256 ok"
}

function Update-Manifest {
    Write-Host 'Resolving latest upstream releases'
    $manifest = Get-Content -Raw -LiteralPath $ManifestPath | ConvertFrom-Json

    $ytdlp = Invoke-RestMethod -Uri 'https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest' -Headers $Headers
    $tag = $ytdlp.tag_name
    $sums = Get-Text "https://github.com/yt-dlp/yt-dlp/releases/download/$tag/SHA2-256SUMS"
    $line = ($sums -split "`n" | Where-Object { $_ -match '\syt-dlp\.exe\s*$' } | Select-Object -First 1)
    $manifest.tools.'yt-dlp'.version = $tag
    $manifest.tools.'yt-dlp'.url = "https://github.com/yt-dlp/yt-dlp/releases/download/$tag/yt-dlp.exe"
    $manifest.tools.'yt-dlp'.sha256 = Get-HashFrom $line

    $deno = Invoke-RestMethod -Uri 'https://api.github.com/repos/denoland/deno/releases/latest' -Headers $Headers
    $denoTag = $deno.tag_name
    $denoUrl = "https://github.com/denoland/deno/releases/download/$denoTag/deno-x86_64-pc-windows-msvc.zip"
    $manifest.tools.deno.version = $denoTag.TrimStart('v')
    $manifest.tools.deno.url = $denoUrl
    $manifest.tools.deno.sha256 = Get-HashFrom (Get-Text "$denoUrl.sha256sum")

    $ffVersion = (Get-Text 'https://www.gyan.dev/ffmpeg/builds/release-version').Trim()
    $ffUrl = "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-$ffVersion-essentials_build.zip"
    $manifest.tools.ffmpeg.version = $ffVersion
    $manifest.tools.ffmpeg.url = $ffUrl
    $manifest.tools.ffmpeg.sha256 = Get-HashFrom (Get-Text "$ffUrl.sha256")

    $manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ManifestPath -Encoding ASCII
    Write-Host "Updated $ManifestPath"
}

function Install-Tool([string]$Name, $Spec) {
    $marker = Join-Path $OutDir "$Name.sha256"
    $installed = (Test-Path -LiteralPath $marker) -and
        ((Get-Content -LiteralPath $marker -Raw).Trim() -eq $Spec.sha256) -and
        (@($Spec.files | Where-Object { -not (Test-Path -LiteralPath (Join-Path $OutDir $_.to)) }).Count -eq 0)
    if ($installed -and -not $Force) {
        Write-Host "$Name $($Spec.version): already installed"
        return
    }

    Write-Host "$Name $($Spec.version)"
    $work = Join-Path ([IO.Path]::GetTempPath()) ("cadence-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        $download = Join-Path $work 'download'
        Save-Verified $Spec.url $Spec.sha256 $download

        $stage = Join-Path $work 'stage'
        New-Item -ItemType Directory -Path $stage | Out-Null
        if ($Spec.archive) {
            $zip = [IO.Compression.ZipFile]::OpenRead($download)
            try {
                foreach ($file in $Spec.files) {
                    $entry = $zip.Entries | Where-Object { $_.FullName -like $file.from } | Select-Object -First 1
                    if (-not $entry) { throw "$($file.from) is not in the $Name archive" }
                    [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $stage $file.to), $true)
                }
            } finally { $zip.Dispose() }
        } else {
            Copy-Item -LiteralPath $download -Destination (Join-Path $stage $Spec.files[0].to)
        }

        New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
        foreach ($file in $Spec.files) {
            Move-Item -LiteralPath (Join-Path $stage $file.to) -Destination (Join-Path $OutDir $file.to) -Force
        }
        Set-Content -LiteralPath $marker -Value $Spec.sha256 -Encoding ASCII
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($UpdateManifest) { Update-Manifest }

$manifest = Get-Content -Raw -LiteralPath $ManifestPath | ConvertFrom-Json
foreach ($property in $manifest.tools.PSObject.Properties) {
    Install-Tool $property.Name $property.Value
}
Write-Host "Done. Tools are in $OutDir"
