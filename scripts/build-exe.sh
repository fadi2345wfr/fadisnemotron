#!/usr/bin/env bash
# Builds the Windows program as ONE file, with the Android app inside.
# Runs on Windows (Git Bash, as on the GitHub runner) or anywhere with a
# Windows toolchain:
#
#   APK=path/to/FadisNemotron.apk ./scripts/build-exe.sh
#
# The program writes the Android app to the user's data folder when it starts
# and hands it to the phone from there (install page with QR code), so the
# user needs nothing but this one .exe. In CI the APK is required
# (REQUIRE_APK=1): an exe without it cannot be published.
#
# The result lands in dist/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
APK="${APK:-}"

if [ -n "$APK" ]; then
  [ -f "$APK" ] || { echo "APK nicht gefunden: $APK" >&2; exit 1; }
  [ "$(wc -c < "$APK")" -gt 1000000 ] || { echo "APK ist verdächtig klein: $APK" >&2; exit 1; }
  # rustc wants a path it can open: C:/… and not Git Bash's /c/…
  APK_PATH="$(cd "$(dirname "$APK")" && (pwd -W 2>/dev/null || pwd))/$(basename "$APK")"
  echo "==> Baue Fadis Nemotron $VERSION mit Android-App ($APK_PATH)"
  FADIS_APK_PATH="$APK_PATH" cargo build --release --locked --features embed-apk
elif [ "${REQUIRE_APK:-0}" = "1" ]; then
  echo "REQUIRE_APK=1, aber APK ist nicht gesetzt." >&2
  exit 1
else
  echo "==> Baue Fadis Nemotron $VERSION ohne Android-App (APK nicht gesetzt)"
  cargo build --release --locked
fi

DIST="$ROOT/dist"
rm -rf "$DIST"
mkdir -p "$DIST"
EXE="$DIST/FadisNemotron-$VERSION-windows.exe"
cp target/release/fadis-nemotron.exe "$EXE"

if [ -n "$APK" ]; then
  exe_size="$(wc -c < "$EXE")"
  apk_size="$(wc -c < "$APK")"
  # The app is inside the file: the exe must be at least as big as the APK.
  [ "$exe_size" -gt "$apk_size" ] || { echo "Die Exe ($exe_size Bytes) ist nicht größer als die APK ($apk_size Bytes)." >&2; exit 1; }
fi
(cd "$DIST" && sha256sum "$(basename "$EXE")" > "$(basename "$EXE").sha256")
ls -la "$DIST"
cat "$EXE.sha256"
