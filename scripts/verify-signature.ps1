#requires -Version 5.1
<#
.SYNOPSIS
    Fail unless every supplied file has a valid Authenticode signature.

.DESCRIPTION
    Used by the tag-triggered release workflow after SignPath returns the
    signed artifacts. A valid status proves Windows can build the signer
    chain and that the file has not been altered since signing. Public
    release signatures must also carry an RFC 3161 timestamp so they remain
    verifiable after the short-lived signing certificate expires.

.PARAMETER Path
    One or more signed files to inspect.

.PARAMETER AllowMissingTimestamp
    Permit a valid signature without a countersignature. This exists for
    local diagnostics; release verification must not use it.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string[]]$Path,
    [switch]$AllowMissingTimestamp
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

foreach ($file in $Path) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "signed artifact not found: $file"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $file
    if ($signature.Status -ne 'Valid') {
        throw "invalid Authenticode signature on ${file}: $($signature.Status) - $($signature.StatusMessage)"
    }
    if ($null -eq $signature.TimeStamperCertificate -and -not $AllowMissingTimestamp) {
        throw "signature on $file has no timestamp"
    }
    $signer = $signature.SignerCertificate.Subject
    Write-Host "ok    $file"
    Write-Host "      signer: $signer"
    if ($null -ne $signature.TimeStamperCertificate) {
        Write-Host "      timestamp: $($signature.TimeStamperCertificate.Subject)"
    }
}
