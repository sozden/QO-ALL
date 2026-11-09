# Builds the 32-bit QO-ALL and packs it into dist\QO-ALL-<version>-windows-x86.zip.
#
# Analog Devices publishes libiio for 64-bit Windows only; the 32-bit libiio.dll is
# built from source with tools/build-libiio-w32.sh (MinGW-w64 on Ubuntu or WSL), with
# libxml2 and libusb inside it, so it needs no other DLL. The Visual C++ runtime is
# linked into qo-all.exe (crt-static).
#
#   rustup target add i686-pc-windows-msvc
#   powershell -ExecutionPolicy Bypass -File tools\package-win32.ps1

param(
    [string]$LibIio = "$PSScriptRoot\..\dist\libiio-w32\libiio.dll"
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$name = "QO-ALL-$version-windows-x86"
$out = Join-Path $root "dist\$name"
$target = "i686-pc-windows-msvc"

$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --target $target
$rc = $LASTEXITCODE
Remove-Item Env:\RUSTFLAGS
if ($rc -ne 0) { throw "build failed" }

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "target\$target\release\qo-all.exe" $out
Copy-Item $LibIio $out
Copy-Item LICENSE (Join-Path $out "LICENSE.txt")
Copy-Item README.md $out
@"
QO-ALL is free software under the GNU General Public License, version 3 or later
(LICENSE.txt). This 32-bit package also carries libiio.dll, built from the unchanged
sources below by tools/build-libiio-w32.sh in QO-ALL's repository, with libxml2 and
libusb linked into it:

libiio 0.26 - GNU LGPL 2.1 or later
  https://github.com/analogdevicesinc/libiio
libusb 1.0.27 - GNU LGPL 2.1 or later
  https://github.com/libusb/libusb
libxml2 2.12.9 - MIT License
  https://gitlab.gnome.org/GNOME/libxml2

FT8/FT4: the LDPC code tables, CRC and message encoding follow WSJT-X (GPL-3.0),
https://sourceforge.net/projects/wsjt/ - by Joe Taylor K1JT, Steve Franke K9AN,
Bill Somerville G4WJS and the WSJT-X team.
"@ | Set-Content -Encoding ascii (Join-Path $out "THIRD-PARTY.txt")

$zip = Join-Path $root "dist\$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $out -DestinationPath $zip
Write-Host "Packaged $zip"
