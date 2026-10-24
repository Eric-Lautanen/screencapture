#requires -Version 5.1
<#
.SYNOPSIS
    Download the pinned Windows FFmpeg engine into vendor\ffmpeg.

.DESCRIPTION
    Uses only Windows PowerShell and .NET - no curl, tar, or 7-Zip needed.
    The archive is pinned by URL, byte count, and SHA-256; a mismatch stops
    before anything is extracted. Only ffmpeg.exe, ffprobe.exe, and the
    license text are installed. The engine integrity record is left for
    `screencapture --check` to write, so a deliberate engine update can
    re-record it.

.PARAMETER Destination
    Engine folder. Defaults to vendor\ffmpeg beside this repository.

.PARAMETER Force
    Replace an engine that is already present. Close screencapture first.

.PARAMETER KeepArchive
    Keep the verified archive in the system temp folder.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\fetch-ffmpeg.ps1

.EXAMPLE
    .\scripts\fetch-ffmpeg.cmd -Force
#>
[CmdletBinding()]
param(
    [string]$Destination = (Join-Path (Split-Path -Parent $PSScriptRoot) 'vendor\ffmpeg'),
    [switch]$Force,
    [switch]$KeepArchive
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http
trap {
    Write-Host "error: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

$ArchiveName = 'ffmpeg-n9.0.2-28-g669f5ab083-win64-gpl-9.0.zip'
$ArchiveUrl = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-18-13-01/' + $ArchiveName
$ArchiveBytes = 193978993
$ArchiveSha256 = 'a38198cc130ed3fa02097d0baebcea9c1b8a9d9d3d1d8161d691d1f03709158b'
$EngineVersion = 'FFmpeg 9.0.2 (BtbN autobuild 2026-10-18, win64 GPL)'

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Get-Sha256 {
    param([string]$Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Archive {
    param([string]$Path)

    if (Test-Path -LiteralPath $Path) {
        $existing = Get-Sha256 $Path
        if ($existing -eq $ArchiveSha256) {
            Write-Step "Using verified archive in $Path"
            return
        }
        Remove-Item -LiteralPath $Path -Force
    }

    $part = "$Path.part"
    Remove-Item -LiteralPath $part -Force -ErrorAction SilentlyContinue

    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Write-Step "Downloading $ArchiveName (attempt $attempt of 3)"
            if ($attempt -eq 1) {
                Write-Host "    $ArchiveUrl"
            }

            $client = New-Object System.Net.Http.HttpClient
            $client.Timeout = [TimeSpan]::FromMinutes(30)
            $client.DefaultRequestHeaders.UserAgent.ParseAdd('screencapture-fetch-ffmpeg/1.0')
            try {
                $response = $client.GetAsync(
                    $ArchiveUrl,
                    [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead
                ).GetAwaiter().GetResult()
                $null = $response.EnsureSuccessStatusCode()
                $expected = $response.Content.Headers.ContentLength
                $source = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
                $target = [System.IO.File]::Open(
                    $part,
                    [System.IO.FileMode]::Create,
                    [System.IO.FileAccess]::Write,
                    [System.IO.FileShare]::None
                )
                try {
                    $buffer = New-Object byte[] (1MB)
                    $written = 0L
                    while (($read = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
                        $target.Write($buffer, 0, $read)
                        $written += $read
                        if ($expected -and $expected -gt 0) {
                            $percent = [Math]::Min(100, [int](100 * $written / $expected))
                            Write-Progress -Activity 'Downloading FFmpeg' `
                                -Status "$([Math]::Round($written / 1MB)) MB of $([Math]::Round($expected / 1MB)) MB" `
                                -PercentComplete $percent
                        } else {
                            Write-Progress -Activity 'Downloading FFmpeg' `
                                -Status "$([Math]::Round($written / 1MB)) MB"
                        }
                    }
                } finally {
                    $target.Dispose()
                    $source.Dispose()
                    $response.Dispose()
                }
            } finally {
                $client.Dispose()
                Write-Progress -Activity 'Downloading FFmpeg' -Completed
            }

            $length = (Get-Item -LiteralPath $part).Length
            if ($expected -and $length -ne [long]$expected) {
                throw "download ended at $length bytes, expected $expected"
            }
            if ($length -ne $ArchiveBytes) {
                throw "archive is $length bytes, expected $ArchiveBytes"
            }

            $actual = Get-Sha256 $part
            if ($actual -ne $ArchiveSha256) {
                throw "SHA-256 mismatch: got $actual, expected $ArchiveSha256"
            }

            Move-Item -LiteralPath $part -Destination $Path -Force
            Write-Step "Download verified: $ArchiveSha256"
            return
        } catch {
            Remove-Item -LiteralPath $part -Force -ErrorAction SilentlyContinue
            if ($attempt -eq 3) {
                throw
            }
            Write-Warning "Download failed: $($_.Exception.Message)"
            Start-Sleep -Seconds (2 * $attempt)
        }
    }
}

if ($env:OS -ne 'Windows_NT') {
    throw 'This script installs the Windows engine; use the system FFmpeg on macOS or Linux.'
}
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'The pinned engine is 64-bit; this Windows installation is 32-bit.'
}

$Destination = [System.IO.Path]::GetFullPath($Destination)
# Only the executables are an engine. A fresh checkout already carries the
# pinned license text and integrity record, and those must not look installed.
$targets = @(
    (Join-Path $Destination 'ffmpeg.exe'),
    (Join-Path $Destination 'ffprobe.exe')
)
$present = @($targets | Where-Object { Test-Path -LiteralPath $_ })
if ($present.Count -gt 0 -and -not $Force) {
    throw "An engine is already installed at $Destination. Pass -Force to replace it (close screencapture first)."
}

Write-Step "Preparing $Destination"
New-Item -ItemType Directory -Path $Destination -Force | Out-Null

$archive = Join-Path ([System.IO.Path]::GetTempPath()) $ArchiveName
Get-Archive $archive

Add-Type -AssemblyName System.IO.Compression.FileSystem
$staging = Join-Path ([System.IO.Path]::GetTempPath()) ('screencapture-ffmpeg-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging -Force | Out-Null
$zip = $null
try {
    Write-Step "Extracting $EngineVersion"
    $zip = [System.IO.Compression.ZipFile]::OpenRead($archive)
    $ffmpeg = $zip.Entries |
        Where-Object { $_.FullName -match '(^|/)bin/ffmpeg\.exe$' } |
        Select-Object -First 1
    $ffprobe = $zip.Entries |
        Where-Object { $_.FullName -match '(^|/)bin/ffprobe\.exe$' } |
        Select-Object -First 1
    $license = $zip.Entries |
        Where-Object { $_.FullName -match '(^|/)LICENSE\.txt$' -and $_.FullName -notmatch '/bin/' } |
        Sort-Object { $_.FullName.Length } |
        Select-Object -First 1

    if (-not $ffmpeg -or -not $ffprobe -or -not $license) {
        throw 'The archive does not contain bin/ffmpeg.exe, bin/ffprobe.exe, and LICENSE.txt.'
    }

    $staged = @(
        @{ Entry = $ffmpeg; Path = (Join-Path $staging 'ffmpeg.exe') },
        @{ Entry = $ffprobe; Path = (Join-Path $staging 'ffprobe.exe') },
        @{ Entry = $license; Path = (Join-Path $staging 'ffmpeg.LICENSE.txt') }
    )
    foreach ($item in $staged) {
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($item.Entry, $item.Path, $true)
        if ((Get-Item -LiteralPath $item.Path).Length -le 0) {
            throw "Extracted $($item.Entry.FullName) is empty."
        }
    }

    # A new engine invalidates the old record; --check writes the new one.
    Remove-Item -LiteralPath (Join-Path $Destination 'engine.sha256') -Force -ErrorAction SilentlyContinue
    foreach ($item in $staged) {
        $name = Split-Path -Leaf $item.Path
        try {
            Copy-Item -LiteralPath $item.Path -Destination (Join-Path $Destination $name) -Force
        } catch {
            throw "Cannot install $name (close screencapture and retry): $($_.Exception.Message)"
        }
    }
} finally {
    if ($zip) { $zip.Dispose() }
    Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
    if (-not $KeepArchive) {
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
    }
}

$ffmpegPath = Join-Path $Destination 'ffmpeg.exe'
Write-Step "Installed $EngineVersion"
Write-Host "    $ffmpegPath"
Write-Host ''
Write-Host 'Next: run `screencapture --check` (or `cargo run --release -- --check`)'
Write-Host 'to record engine.sha256 and verify the engine.'
