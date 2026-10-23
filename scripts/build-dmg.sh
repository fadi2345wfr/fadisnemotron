#!/usr/bin/env bash
# Builds "Fadis Nemotron.app" and packs it into a DMG. Runs on macOS.
#
#   ./scripts/build-dmg.sh                         # Intel (x86_64), the default
#   TARGET=aarch64-apple-darwin ./scripts/build-dmg.sh   # Apple Silicon (M1 and later)
#   APK=path/to/FadisNemotron.apk ./scripts/build-dmg.sh  # with the Android app inside
#
# With APK the Android app goes into the app bundle (Contents/Resources), and
# the Mac app hands it to the phone: no download from GitHub needed. In CI the
# APK is required (REQUIRE_APK=1), so a DMG without it cannot be published.
#
# The result lands in dist/.
set -euo pipefail

TARGET="${TARGET:-x86_64-apple-darwin}"
APP_NAME="Fadis Nemotron"
BIN="fadis-nemotron"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
case "$TARGET" in
  # Catalina covers every Intel Mac that still gets security updates.
  x86_64-*) ARCH_LABEL="intel"; LIPO_ARCH="x86_64"; DEFAULT_MIN_MACOS="10.15" ;;
  # The first Apple Silicon Macs came with Big Sur.
  aarch64-*) ARCH_LABEL="apple-silicon"; LIPO_ARCH="arm64"; DEFAULT_MIN_MACOS="11.0" ;;
  *) echo "Unbekanntes Ziel: $TARGET" >&2; exit 1 ;;
esac
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-$DEFAULT_MIN_MACOS}"

echo "==> Baue $APP_NAME $VERSION für $TARGET (ab macOS $MACOSX_DEPLOYMENT_TARGET)"
rustup target add "$TARGET" >/dev/null
cargo build --release --locked --target "$TARGET"

DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
rm -rf "$DIST"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "target/$TARGET/release/$BIN" "$APP/Contents/MacOS/$BIN"
sed -e "s/__VERSION__/$VERSION/g" -e "s/__MIN_MACOS__/$MACOSX_DEPLOYMENT_TARGET/g" \
  packaging/macos/Info.plist > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"

echo "==> Icon"
ICONSET="$DIST/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" assets/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

echo "==> Android-App einpacken"
APK="${APK:-}"
if [ -n "$APK" ]; then
  # A relative path counts from the repository root.
  [ -f "$APK" ] || { echo "APK nicht gefunden: $APK" >&2; exit 1; }
  [ "$(wc -c < "$APK")" -gt 1000000 ] || { echo "APK ist verdächtig klein: $APK" >&2; exit 1; }
  cp "$APK" "$APP/Contents/Resources/FadisNemotron.apk"
elif [ "${REQUIRE_APK:-0}" = "1" ]; then
  echo "REQUIRE_APK=1, aber APK ist nicht gesetzt." >&2
  exit 1
else
  echo "Ohne Android-App (APK nicht gesetzt): die Mac-App verweist dann auf GitHub."
fi

echo "==> Architektur prüfen"
ARCHS="$(lipo -archs "$APP/Contents/MacOS/$BIN")"
echo "Binary-Architektur: $ARCHS"
if [ "$ARCHS" != "$LIPO_ARCH" ]; then
  echo "Erwartet $LIPO_ARCH, gefunden $ARCHS" >&2
  exit 1
fi

echo "==> Signieren (ad hoc, mit fester Anforderung)"
# No Developer ID here, so Gatekeeper still asks on first launch (see README).
#
# macOS remembers a permission (screen recording, accessibility) by the
# "designated requirement" of the signature. Ad hoc, that is the hash of this
# very build: every update then looked like another program, and the
# permission was lost although its switch in the settings still said "on".
# A fixed requirement (the bundle identifier) keeps the permission across updates.
IDENTIFIER="de.fadi.FadisNemotron"
codesign --force --deep --sign - --identifier "$IDENTIFIER" \
  --requirements "=designated => identifier \"$IDENTIFIER\"" "$APP"
codesign --verify --strict --verbose=2 "$APP"
REQUIREMENT="$(codesign -d -r- "$APP" 2>&1 | grep '^designated' || true)"
echo "$REQUIREMENT"
case "$REQUIREMENT" in
  *"identifier \"$IDENTIFIER\""*) ;;
  *) echo "Die feste Anforderung fehlt in der Signatur." >&2; exit 1 ;;
esac
case "$REQUIREMENT" in
  *cdhash*) echo "Die Signatur hängt noch am Hash dieses Builds." >&2; exit 1 ;;
esac

echo "==> DMG"
STAGE="$DIST/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
cp packaging/macos/SO-OEFFNEST-DU-DIE-APP.txt "$STAGE/So öffnest du die App.txt"
DMG="$DIST/FadisNemotron-$VERSION-$ARCH_LABEL.dmg"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG"
rm -rf "$STAGE"
hdiutil verify "$DMG"
(cd "$DIST" && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256")

echo "==> Fertig: $DMG"
cat "$DMG.sha256"
