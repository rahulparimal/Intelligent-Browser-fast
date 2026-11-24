# One-line installer for Windows (PowerShell):
#   irm https://raw.githubusercontent.com/rahulparimal/Intelligent-Browser-fast/main/scripts/install.ps1 | iex
# Downloads the latest Setup .exe, checks its SHA-256, and runs it. Use -Silent for no wizard. No administrator rights needed.
# NOT RUN YET: written on Linux, never executed on Windows.
param([string]$Version = "", [switch]$Silent, [string]$Repo = "rahulparimal/Intelligent-Browser-fast")
$ErrorActionPreference = "Stop"
if (-not $Version) {
    $latest = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -MaximumRedirection 0 -SkipHttpErrorCheck -ErrorAction SilentlyContinue
    $tag = ($latest.Headers.Location | Select-Object -First 1) -replace '.*/tag/', ''
    if (-not $tag) { throw "Could not find the latest release of $Repo (has one been published?)." }
    $Version = $tag.TrimStart('v')
}
$base = "https://github.com/$Repo/releases/download/v$Version"
$file = "Intelli-Rust-$Version-Setup.exe"
$tmp = Join-Path $env:TEMP "intelli-install"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
Write-Host "==> Downloading $file"
Invoke-WebRequest -Uri "$base/$file" -OutFile (Join-Path $tmp $file)
Invoke-WebRequest -Uri "$base/SHA256SUMS-windows-x64.txt" -OutFile (Join-Path $tmp "SHA256SUMS.txt")
Write-Host "==> Checking the download"
$line = Get-Content (Join-Path $tmp "SHA256SUMS.txt") | Where-Object { $_ -match [regex]::Escape($file) } | Select-Object -First 1
if (-not $line) { throw "No checksum listed for $file." }
$expected = ($line -split '\s+')[0].ToLower()
$actual = (Get-FileHash (Join-Path $tmp $file) -Algorithm SHA256).Hash.ToLower()
if ($expected -ne $actual) { throw "Checksum mismatch: refusing to install." }
Write-Host "==> Running the installer"
if ($Silent) { Start-Process -Wait -FilePath (Join-Path $tmp $file) -ArgumentList "/VERYSILENT", "/NORESTART" }
else { Start-Process -Wait -FilePath (Join-Path $tmp $file) }
Write-Host "Done. Open 'Intelli Search' from the Start menu."
