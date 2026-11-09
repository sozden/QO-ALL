#!/bin/bash
# Cross-builds a 32-bit libiio.dll (0.26, network + USB backends) with libxml2 and
# libusb linked into it statically, using MinGW-w64 i686 on Ubuntu.
#
#   sudo apt install gcc-mingw-w64-i686 g++-mingw-w64-i686 cmake autoconf automake libtool
#   mkdir -p ~/w32/src && cd ~/w32/src
#   curl -LO https://github.com/analogdevicesinc/libiio/archive/refs/tags/v0.26.tar.gz
#   curl -LO https://github.com/libusb/libusb/releases/download/v1.0.27/libusb-1.0.27.tar.bz2
#   curl -LO https://download.gnome.org/sources/libxml2/2.12/libxml2-2.12.9.tar.xz
#   bash tools/build-libiio-w32.sh
#   i686-w64-mingw32-strip -o dist/libiio-w32/libiio.dll \
#       ~/w32/build/libiio-0.26/build/libiio.dll
#
# sha256: v0.26.tar.gz fb445fb8…7be5, libusb ffaa41d7…1575, libxml2 59912db5…f483590
set -euo pipefail
H=i686-w64-mingw32
W=~/w32
P=$W/prefix
rm -rf $W/build $P
mkdir -p $W/build $P
cd $W/build

for f in $W/src/*.tar.*; do
    echo "== $f: $(tar tf "$f" | wc -l) files, $(tar tf "$f" | grep -ciE '\.(exe|dll|bat|ps1)$' || true) exe/dll/bat/ps1"
    tar xf "$f"
done

echo "== libusb"
cd $W/build/libusb-1.0.27
./configure --host=$H --prefix=$P --enable-static --disable-shared >/dev/null
make -j"$(nproc)" >/dev/null
make install >/dev/null

echo "== libxml2"
cd $W/build/libxml2-2.12.9
./configure --host=$H --prefix=$P --enable-static --disable-shared --without-python \
    --without-iconv --without-lzma --without-zlib --without-icu --without-http \
    --without-ftp --without-threads --without-modules --without-debug >/dev/null
make -j"$(nproc)" >/dev/null
make install >/dev/null

echo "== libiio"
# The sources name some Windows headers with capitals; MinGW's are lower case.
mkdir -p $W/caseinc
for h in windows winsock2 ws2tcpip; do
    ln -sf /usr/$H/include/$h.h $W/caseinc/"$(echo ${h:0:1} | tr a-z A-Z)${h:1}".h
done
ln -sf /usr/$H/include/ws2tcpip.h $W/caseinc/Ws2tcpip.h
cd $W/build/libiio-0.26
cat > $W/build/toolchain.cmake <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86)
set(CMAKE_C_COMPILER $H-gcc)
set(CMAKE_CXX_COMPILER $H-g++)
set(CMAKE_RC_COMPILER $H-windres)
set(CMAKE_FIND_ROOT_PATH /usr/$H $P)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF
mkdir -p build && cd build
PKG_CONFIG_LIBDIR=$P/lib/pkgconfig cmake .. -DCMAKE_TOOLCHAIN_FILE=$W/build/toolchain.cmake \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=$P \
    -DWITH_NETWORK_BACKEND=ON -DWITH_USB_BACKEND=ON -DWITH_XML_BACKEND=ON \
    -DWITH_LOCAL_BACKEND=OFF -DWITH_SERIAL_BACKEND=OFF -DHAVE_DNS_SD=OFF \
    -DWITH_IIOD=OFF -DWITH_TESTS=OFF -DWITH_DOC=OFF -DCSHARP_BINDINGS=OFF \
    -DPYTHON_BINDINGS=OFF -DWITH_ZSTD=OFF \
    -DLIBXML2_INCLUDE_DIR=$P/include/libxml2 -DLIBXML2_LIBRARIES=$P/lib/libxml2.a \
    -DLIBXML2_LIBRARY=$P/lib/libxml2.a \
    -DLIBUSB_INCLUDE_DIR=$P/include/libusb-1.0 -DLIBUSB_LIBRARIES=$P/lib/libusb-1.0.a \
    -DCMAKE_C_FLAGS="-DLIBXML_STATIC -I$W/caseinc" \
    -DCMAKE_SHARED_LINKER_FLAGS="-static-libgcc -static -Wl,--enable-stdcall-fixup" \
    > $W/build/cmake.log 2>&1 || { tail -40 $W/build/cmake.log; exit 1; }
grep -iE "backend|libxml|libusb" $W/build/cmake.log | head -20
make -j"$(nproc)" iio 2>&1 | tail -5
ls -la *.dll
$H-objdump -p libiio.dll | grep -E "DLL Name|Magic" | sort -u
