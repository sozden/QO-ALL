#!/bin/bash
# Builds QO-ALL and packs it into dist/QO-ALL-<version>-linux-x64.tar.gz.
# libiio comes from the distribution's packages (see INSTALL.txt); built on Ubuntu 24.04,
# so the program needs glibc 2.39 or newer (Ubuntu 24.04+, Debian 13+, Mint 22+).
#
#   bash tools/package-linux.sh
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
name="QO-ALL-$version-linux-x64"
out="dist/$name"

cargo build --release
rm -rf "$out" "dist/$name.tar.gz"
mkdir -p "$out"
cp target/release/qo-all "$out/"
strip "$out/qo-all"
cp README.md CHANGELOG.md "$out/"
cp LICENSE "$out/LICENSE.txt"
cp assets/qo-all-icon.png "$out/qo-all.png"
cat > "$out/qo-all.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=QO-ALL
Comment=QO-100 SSB, CW and FT8/FT4 transceiver and receiver
Exec=qo-all
Icon=qo-all
Terminal=false
Categories=HamRadio;AudioVideo;
EOF
# The ADALM-Pluto on USB for a normal user (USB 0456:b673).
cat > "$out/53-adi-plutosdr-usb.rules" <<'EOF'
SUBSYSTEM=="usb", ATTRS{idVendor}=="0456", ATTRS{idProduct}=="b673", MODE="0666"
EOF
cat > "$out/INSTALL.txt" <<'EOF'
QO-ALL for Linux (x86-64, glibc 2.39+: Ubuntu 24.04 or newer, Debian 13, Mint 22)

1. libiio and the sound system (Ubuntu / Debian):

   sudo apt install libiio0 libasound2t64

   (On older names: libasound2.) A Pluto on Ethernet needs nothing more.
   For an RTL-SDR or a HackRF (receive only), their libraries too; they bring the
   udev rules that let a normal user open them:

   sudo apt install librtlsdr2 libhackrf0

   (librtlsdr0 on older systems such as Debian 12.)

   An RTL-SDR taken by the TV driver (dvb_usb_rtl28xxu) needs it blacklisted:
   echo blacklist dvb_usb_rtl28xxu | sudo tee /etc/modprobe.d/blacklist-rtl.conf

2. A Pluto on USB: let a normal user open it, then unplug and replug it:

   sudo cp 53-adi-plutosdr-usb.rules /etc/udev/rules.d/
   sudo udevadm control --reload-rules

3. Run ./qo-all from this folder. To have it in the menu:

   install -Dm755 qo-all ~/.local/bin/qo-all
   install -Dm644 qo-all.png ~/.local/share/icons/hicolor/256x256/apps/qo-all.png
   install -Dm644 qo-all.desktop ~/.local/share/applications/qo-all.desktop

The radio is chosen under Settings (e.g. 192.168.2.1, or Search USB). Settings are kept
in ~/.config/qo-all/settings.txt, the log in ~/.config/qo-all/qo-all.adi.
`./qo-all --diagnose 192.168.2.1` prints what the program finds without the window.

---

QO-ALL Linux (x86-64, glibc 2.39+: Ubuntu 24.04 ve sonrası, Debian 13, Mint 22)

1. sudo apt install libiio0 libasound2t64
   (RTL-SDR ve HackRF için ayrıca: sudo apt install librtlsdr2 libhackrf0)
2. USB'li Pluto için yukarıdaki udev kuralını kopyalayın, cihazı çıkarıp takın.
3. ./qo-all ile çalıştırın; menüye eklemek için yukarıdaki install komutları.
EOF
chmod 644 "$out"/*
chmod 755 "$out/qo-all"
tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "Packaged dist/$name.tar.gz"
