<#
.SYNOPSIS
  Builds tail.exe in release mode and packs it into an MSIX.

.DESCRIPTION
  Stages tail.exe, the logo assets and AppxManifest.xml (with its {{...}}
  tokens filled) under target\msix\stage, then runs makeappx. The package
  version comes from Cargo.toml: MSIX needs a numeric quad, so
  "1.0.0-beta.1" becomes "1.0.0.0". The fourth field stays 0 because the
  Microsoft Store reserves it. A `version` under [package.metadata.msix]
  overrides it (betas use 0.99.N.0, see Cargo.toml). The .msix file name
  keeps the full crate version (tail-win_1.0.0-beta.1_x64.msix).

  The identity defaults are the Partner Center values of the reserved Store
  product "tail for Windows" (Product management > Product identity); the
  manifest DisplayName must stay equal to that reserved name.

  The package is unsigned unless -CertificateThumbprint or -PfxPath is given.
  For a Store submission leave it unsigned: Partner Center signs it. To sign a
  sideload build, the certificate subject must equal -Publisher.

  After packing, the Windows App Certification Kit (the same checks Partner
  Center runs) tests the package and writes target\msix\wack-report.xml.
  appcert.exe needs elevation, so this shows a UAC prompt; the script fails
  when the kit reports FAIL. -SkipCertification skips it.

.EXAMPLE
  .\packaging\msix\build-msix.ps1
.EXAMPLE
  .\packaging\msix\build-msix.ps1 -SkipCertification
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
    [switch]$SkipBuild,
    [switch]$SkipCertification
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

# Optional override: `version` under [package.metadata.msix] (beta builds).
if ($cargoToml -match '(?ms)^\[package\.metadata\.msix\][^\[]*?^version\s*=\s*"([^"]*)"') {
    $msixVersion = $Matches[1]
    if ($msixVersion -notmatch '^\d+\.\d+\.\d+\.0$') {
        throw "[package.metadata.msix] version must be a numeric quad ending in .0, got '$msixVersion'."
    }
}

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

# The file name keeps the crate version (with any pre-release tag) so a beta
# build is recognizable; the manifest can only carry the numeric quad.
$msix = Join-Path $out "tail-win_${crateVersion}_$Arch.msix"
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

if (-not $SkipCertification) {
    $appcert = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\App Certification Kit\appcert.exe'
    if (-not (Test-Path $appcert)) { throw "$appcert not found. Install the Windows SDK or pass -SkipCertification." }
    $report = Join-Path $out 'wack-report.xml'
    Remove-Item $report -ErrorAction SilentlyContinue
    # appcert.exe demands elevation; "reset" clears the state of a previous run.
    $cmd = "`"`"$appcert`" reset && `"$appcert`" test -appxpackagepath `"$msix`" -reportoutputpath `"$report`"`""
    Write-Host 'Running the Windows App Certification Kit (elevated, takes a few minutes)...'
    Start-Process cmd.exe -ArgumentList "/c $cmd" -Verb RunAs -Wait -WindowStyle Hidden
    if (-not (Test-Path $report)) { throw "The certification kit wrote no report ($report)." }

    $xml = [xml](Get-Content $report -Raw)
    $overall = $xml.REPORT.OVERALL_RESULT
    foreach ($test in $xml.SelectNodes('//TEST')) {
        $result = $test.SelectSingleNode('RESULT').InnerText
        if ($result -ne 'PASS') { Write-Host "  [$result] $($test.GetAttribute('NAME'))" }
    }
    Write-Host "Certification: $overall (report: $report)"
    if ($overall -eq 'FAIL') { throw 'The Windows App Certification Kit reported FAIL.' }
}
