# Builds QO-ALL and puts it with the libraries it needs into
# dist\QO-ALL-<version>-windows-x64.zip, which runs without installing anything.
#
# libiio and libusb come from PothosSDR (https://downloads.myriadrf.org/builds/PothosSDR/);
# PothosSDR's libiio needs libxml2, which it does not ship: that and its own
# dependencies are taken from SatDump's folder (or give another with -XmlDir).
#
#   powershell -ExecutionPolicy Bypass -File tools\package-windows.ps1

param(
    [string]$Pothos = "$env:ProgramFiles\PothosSDR",
    [string]$XmlDir = "$env:ProgramFiles\SatDump\bin"
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$name = "QO-ALL-$version-windows-x64"
$out = Join-Path $root "dist\$name"

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "build failed" }

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item target\release\qo-all.exe $out

$bin = Join-Path $Pothos "bin"
foreach ($dll in "libiio.dll", "libusb-1.0.dll", "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll") {
    Copy-Item (Join-Path $bin $dll) $out
}
foreach ($dll in "libxml2.dll", "iconv-2.dll", "liblzma.dll", "zlib1.dll") {
    Copy-Item (Join-Path $XmlDir $dll) $out
}

Copy-Item LICENSE (Join-Path $out "LICENSE.txt")
Copy-Item tools\THIRD-PARTY.txt $out
Copy-Item README.md $out

$zip = Join-Path $root "dist\$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $out -DestinationPath $zip
Write-Host "Packaged $zip"
