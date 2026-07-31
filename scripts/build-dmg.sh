#!/usr/bin/env bash
# T068: package Blaze.app into a distributable DMG.
#
#   ./scripts/build-dmg.sh --unsigned                 # dev DMG, ad-hoc signed
#   ./scripts/build-dmg.sh --sign "Developer ID Application: ..." \
#                          [--notarize-profile PROFILE]   # distribution
#
# The app embeds a universal (arm64 + x86_64) Rust core. Uses create-dmg when
# installed (brew install create-dmg) and falls back to hdiutil otherwise.
set -euo pipefail
cd "$(dirname "$0")/.."

MODE="unsigned"
IDENTITY=""
NOTARIZE_PROFILE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --unsigned) MODE="unsigned"; shift ;;
    --sign) MODE="sign"; IDENTITY="$2"; shift 2 ;;
    --notarize-profile) NOTARIZE_PROFILE="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
DIST=dist
APP="$DIST/Blaze.app"
DMG="$DIST/Blaze-$VERSION.dmg"

echo "==> Rust core (universal release)"
./scripts/build-xcframework.sh release

echo "==> Xcode archive (Release)"
command -v xcodegen >/dev/null || { echo "xcodegen required: brew install xcodegen" >&2; exit 1; }
(cd platforms/macos && xcodegen generate)

rm -rf "$DIST" && mkdir -p "$DIST"
SIGN_ARGS=(CODE_SIGN_IDENTITY="-" CODE_SIGNING_REQUIRED=YES)
[[ "$MODE" == "sign" ]] && SIGN_ARGS=(CODE_SIGN_IDENTITY="$IDENTITY")

xcodebuild -project platforms/macos/Blaze.xcodeproj -scheme Blaze \
  -configuration Release -destination 'platform=macOS' \
  -derivedDataPath "$DIST/DerivedData" \
  MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$VERSION" \
  "${SIGN_ARGS[@]}" build | tail -2

cp -R "$DIST/DerivedData/Build/Products/Release/Blaze.app" "$APP"
rm -rf "$DIST/DerivedData"

if [[ "$MODE" == "sign" ]]; then
  echo "==> Deep signing with: $IDENTITY"
  codesign --force --deep --options runtime --timestamp -s "$IDENTITY" "$APP"
  codesign --verify --deep --strict "$APP"
fi

echo "==> DMG"
rm -f "$DMG"
if command -v create-dmg >/dev/null; then
  create-dmg \
    --volname "Blaze" \
    --window-size 540 380 \
    --icon-size 128 \
    --icon "Blaze.app" 140 180 \
    --app-drop-link 400 180 \
    --hide-extension "Blaze.app" \
    "$DMG" "$APP"
else
  echo "    (create-dmg not installed — hdiutil fallback; brew install create-dmg for the styled DMG)"
  STAGE="$DIST/dmg-stage"
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "Blaze" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGE"
fi

if [[ "$MODE" == "sign" ]]; then
  codesign --force --timestamp -s "$IDENTITY" "$DMG"
  if [[ -n "$NOTARIZE_PROFILE" ]]; then
    echo "==> Notarizing (profile: $NOTARIZE_PROFILE)"
    xcrun notarytool submit "$DMG" --keychain-profile "$NOTARIZE_PROFILE" --wait
    xcrun stapler staple "$DMG"
  fi
fi

echo "Done: $DMG"
[[ "$MODE" == "unsigned" ]] && echo "NOTE: unsigned dev build — right-click > Open on first launch (Gatekeeper)."
