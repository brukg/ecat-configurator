#!/bin/sh
# Builds dist/EtherCAT-Configurator-<version>-macos.zip containing "EtherCAT Configurator.app".
# Run on macOS (used by CI). Needs sips and iconutil, which ship with macOS.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
APP=$(cd "$HERE/.." && pwd)
cd "$APP"
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)
cargo build --release --locked
BUNDLE="target/macos/EtherCAT Configurator.app"
rm -rf "target/macos"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp target/release/ecat-configurator "$BUNDLE/Contents/MacOS/"
sed "s/@VERSION@/$VERSION/g" "$HERE/macos/Info.plist" > "$BUNDLE/Contents/Info.plist"
# icon: draw PNGs and convert them to .icns
ICONSET="target/macos/ecat-configurator.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  python3 "$HERE/make_icon.py" "$ICONSET/icon_${s}x${s}.png" "$s"
  python3 "$HERE/make_icon.py" "$ICONSET/icon_${s}x${s}@2x.png" "$((s * 2))"
done
iconutil -c icns "$ICONSET" -o "$BUNDLE/Contents/Resources/ecat-configurator.icns"
mkdir -p dist
OUT="dist/EtherCAT-Configurator-${VERSION}-macos.zip"
rm -f "$OUT"
(cd target/macos && ditto -c -k --keepParent "EtherCAT Configurator.app" "../../$OUT")
echo "App bundle written to $OUT"
