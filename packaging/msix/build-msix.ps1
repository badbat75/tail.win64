<#
.SYNOPSIS
  Builds tail.exe in release mode and packs it into an MSIX.

.DESCRIPTION
  Stages tail.exe, the logo assets and AppxManifest.xml (with its {{...}}
  tokens filled) under target\msix\stage, then runs makeappx. The package
  version comes from Cargo.toml: MSIX needs a numeric quad, so
  "1.0.0-beta.1" becomes "1.0.0.0". The fourth field stays 0 because the
  Microsoft Store reserves it.

  The identity defaults are the Partner Center values of the reserved Store
  product "tail for Windows" (Product management > Product identity); the
  manifest DisplayName must stay equal to that reserved name.

  The package is unsigned unless -CertificateThumbprint or -PfxPath is given.
  For a Store submission leave it unsigned: Partner Center signs it. To sign a
  sideload build, the certificate subject must equal -Publisher.

.EXAMPLE
  .\packaging\msix\build-msix.ps1
.EXAMPLE
  .\packaging\msix\build-msix.ps1 -CertificateThumbprint 0123ABCD...
#>
[CmdletBinding()]
param(
    [string]$IdentityName = 'BadBat75.tailforWindows',
    [string]$Publisher = 'CN=932406D5-4DDE-483C-9D6C-7517FB42206B',
    [string]$PublisherDisplayName = 'BadBat75',
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64',
    [string]$CertificateThumbprint,
    [string]$PfxPath,
    [SecureString]$PfxPassword,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$here = $PSScriptRoot

function Find-SdkTool([string]$name) {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $tool = Get-ChildItem -Path $kits -Filter $name -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Directory.Name -eq 'x64' } |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $tool) { throw "$name not found under $kits. Install the Windows 10/11 SDK." }
    $tool.FullName
}

# Version: first `version = "..."` in Cargo.toml ([package] comes first).
$cargoToml = Get-Content (Join-Path $root 'Cargo.toml') -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)[^"]*"') {
    throw 'Cannot read the package version from Cargo.toml.'
}
$crateVersion = ($Matches[0] -replace '^version\s*=\s*"|"$', '')
$msixVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"

$triple = if ($Arch -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
if (-not $SkipBuild) {
    cargo build --release --target $triple --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }
}
$exe = Join-Path $root "target\$triple\release\tail.exe"
if (-not (Test-Path $exe)) { throw "$exe not found; run without -SkipBuild." }

$out = Join-Path $root 'target\msix'
$stage = Join-Path $out 'stage'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $stage 'Assets') | Out-Null

Copy-Item $exe $stage
Copy-Item (Join-Path $root 'LICENSE') $stage -ErrorAction SilentlyContinue
Copy-Item (Join-Path $here 'Assets\*.png') (Join-Path $stage 'Assets')

$manifest = (Get-Content (Join-Path $here 'AppxManifest.xml') -Raw).
    Replace('{{IDENTITY_NAME}}', $IdentityName).
    Replace('{{PUBLISHER}}', [Security.SecurityElement]::Escape($Publisher)).
    Replace('{{PUBLISHER_DISPLAY_NAME}}', [Security.SecurityElement]::Escape($PublisherDisplayName)).
    Replace('{{VERSION}}', $msixVersion).
    Replace('{{ARCH}}', $Arch)
if ($manifest -match '\{\{\w+\}\}') { throw "Unfilled manifest token: $($Matches[0])" }
[IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'), $manifest, [Text.UTF8Encoding]::new($false))

$msix = Join-Path $out "tail-win_${msixVersion}_$Arch.msix"
& (Find-SdkTool 'makeappx.exe') pack /o /h SHA256 /d $stage /p $msix
if ($LASTEXITCODE -ne 0) { throw 'makeappx pack failed.' }

if ($CertificateThumbprint -or $PfxPath) {
    $signtool = Find-SdkTool 'signtool.exe'
    $signArgs = @('sign', '/fd', 'SHA256')
    if ($CertificateThumbprint) {
        $signArgs += @('/sha1', $CertificateThumbprint)
    } else {
        $signArgs += @('/f', $PfxPath)
        if ($PfxPassword) {
            $signArgs += @('/p', [Net.NetworkCredential]::new('', $PfxPassword).Password)
        }
    }
    & $signtool @signArgs $msix
    if ($LASTEXITCODE -ne 0) { throw 'signtool sign failed. The certificate subject must equal -Publisher.' }
}

Write-Host "tail-win $crateVersion -> $msix (MSIX version $msixVersion)"
