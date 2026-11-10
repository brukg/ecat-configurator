#!/bin/sh
# Builds dist/EtherCAT-Configurator-<version>-windows.zip (program, samples, docs).
# Run in a bash shell on Windows (used by CI, Git Bash).
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
APP=$(cd "$HERE/.." && pwd)
cd "$APP"
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)
cargo build --release --locked
STAGE="target/windows/EtherCAT-Configurator-$VERSION"
rm -rf target/windows
mkdir -p "$STAGE/samples" "$STAGE/docs"
cp target/release/ecat-configurator.exe "$STAGE/"
cp samples/*.xml "$STAGE/samples/"
cp README.md LICENSE "$STAGE/"
cp docs/USER_GUIDE.md "$STAGE/docs/"
mkdir -p dist
OUT="dist/EtherCAT-Configurator-${VERSION}-windows.zip"
rm -f "$OUT"
(cd target/windows && 7z a -tzip "../../$OUT" "EtherCAT-Configurator-$VERSION" > /dev/null)
echo "Zip written to $OUT"
