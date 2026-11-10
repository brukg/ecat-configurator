#!/bin/sh
# Builds dist/ecat-configurator_<version>_<arch>.deb from this project.
# Usage: packaging/build-deb.sh   (run from the app folder or anywhere)
set -e
umask 022
HERE=$(cd "$(dirname "$0")" && pwd)
APP=$(cd "$HERE/.." && pwd)
cd "$APP"

NAME=ecat-configurator
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)
ARCH=$(dpkg --print-architecture)
MAINTAINER="${DEB_MAINTAINER:-EtherCAT Configurator Maintainers <maintainer@example.com>}"

cargo build --release

STAGE="$APP/target/deb/${NAME}_${VERSION}_${ARCH}"
rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/$NAME/samples" "$STAGE/usr/share/doc/$NAME"

install -m 0755 "target/release/$NAME" "$STAGE/usr/bin/$NAME"
strip --strip-unneeded "$STAGE/usr/bin/$NAME" 2>/dev/null || true
install -m 0644 "$HERE/$NAME.desktop" "$STAGE/usr/share/applications/$NAME.desktop"
for s in 48 64 128 256; do
    d="$STAGE/usr/share/icons/hicolor/${s}x${s}/apps"
    mkdir -p "$d"
    python3 "$HERE/make_icon.py" "$d/$NAME.png" "$s"
    chmod 0644 "$d/$NAME.png"
done
install -m 0644 samples/*.xml "$STAGE/usr/share/$NAME/samples/"

cat > "$STAGE/usr/share/doc/$NAME/copyright" <<COPY
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: $NAME
Files: *
Copyright: $(date +%Y) the $NAME authors
License: proprietary
 All rights reserved.
COPY
gzip -9n -c > "$STAGE/usr/share/doc/$NAME/changelog.gz" <<LOG
$NAME ($VERSION) stable; urgency=medium

  * Release $VERSION.

 -- $MAINTAINER  $(date -R)
LOG
chmod 0644 "$STAGE/usr/share/doc/$NAME/"*

# shared-library dependencies of the binary, worked out by dpkg-shlibdeps
TMP=$(mktemp -d)
mkdir -p "$TMP/debian"
printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$NAME" "$NAME" > "$TMP/debian/control"
DEPS=$(cd "$TMP" && dpkg-shlibdeps -O "$STAGE/usr/bin/$NAME" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
rm -rf "$TMP"
[ -n "$DEPS" ] || DEPS="libc6, libgcc-s1"
# libraries opened at run time (window system, OpenGL, file dialogs)
DEPS="$DEPS, libgl1, libegl1, libxkbcommon0, libx11-6, libxcursor1, libxrandr2, libxi6, libwayland-client0"

find "$STAGE" -type d -exec chmod 0755 {} +
SIZE=$(du -sk "$STAGE/usr" | cut -f1)
cat > "$STAGE/DEBIAN/control" <<CTRL
Package: $NAME
Version: $VERSION
Section: devel
Priority: optional
Architecture: $ARCH
Maintainer: $MAINTAINER
Installed-Size: $SIZE
Depends: $DEPS
Recommends: libcap2-bin
Description: EtherCAT network configurator for ESI and ENI files
 Desktop tool to build EtherCAT network configurations: load ESI device
 descriptions into a catalog, assemble the network, edit PDO assignment and
 mapping, CoE start-up commands, distributed clocks and init commands, and
 export or import ENI files. Also shows the process image (C header / CSV
 export), generates slave EEPROM (SII) images and can scan a live bus.
CTRL
install -m 0755 "$HERE/postinst" "$STAGE/DEBIAN/postinst"
install -m 0755 "$HERE/postrm" "$STAGE/DEBIAN/postrm"

mkdir -p "$APP/dist"
OUT="$APP/dist/${NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$STAGE" "$OUT"
echo "Package written to $OUT"
