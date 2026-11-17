#!/bin/zsh
# Builds, packages and signs build/Crawler.app from the Rust/Rune port in crawler-rune/.
# The Swift app is still buildable with scripts/build-swift.sh (build/Crawler-Swift.app).
set -euo pipefail
cd "${0:A:h:h}"
SIGNING_IDENTITY="${CRAWLER_SIGNING_IDENTITY-Crawler Local Development}"
if [[ -z "$SIGNING_IDENTITY" || "$SIGNING_IDENTITY" == "-" ]]; then
  print -u2 "Choose a persistent code-signing certificate. Ad-hoc signing is not supported."
  exit 1
fi
if ! security find-identity -p codesigning | grep -Fq "\"$SIGNING_IDENTITY\""; then
  print -u2 "Missing signing identity: $SIGNING_IDENTITY. Create or restore your persistent code-signing certificate."
  print -u2 "Do not recreate it for each build: changing the certificate resets Crawler's privacy identity."
  exit 1
fi
(cd crawler-rune && cargo build --release --locked)

# Assemble beside the old bundle, then swap, so a running copy never sees half a bundle.
APP="$PWD/build/Crawler.app"
STAGE="$PWD/build/.Crawler.app.staging"
rm -rf "$STAGE"
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources"
# Keep the unstripped executable in crawler-rune/target/release for symbolication.
cp crawler-rune/target/release/crawler "$STAGE/Contents/MacOS/Crawler"
xcrun strip -S -x "$STAGE/Contents/MacOS/Crawler"
cat > "$STAGE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Crawler</string>
<key>CFBundleIdentifier</key><string>studio.crawler.native-prototype</string>
<key>CFBundleName</key><string>Crawler</string>
<key>CFBundleDisplayName</key><string>Crawler</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.2.0</string>
<key>CFBundleVersion</key><string>2</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSDesktopFolderUsageDescription</key><string>Crawler needs access to show and open files and folders on your Desktop.</string>
<key>NSDocumentsFolderUsageDescription</key><string>Crawler needs access to show and open files and folders in Documents.</string>
<key>NSDownloadsFolderUsageDescription</key><string>Crawler needs access to show and open files and folders in Downloads.</string>
<key>NSLocalNetworkUsageDescription</key><string>Crawler discovers nearby AirDrop devices so you can choose where to send files.</string>
<key>NSBonjourServices</key><array><string>_airdrop._tcp</string></array>
<key>CFBundleDocumentTypes</key><array><dict>
  <key>CFBundleTypeName</key><string>Folder</string>
  <key>CFBundleTypeRole</key><string>Viewer</string>
  <key>LSHandlerRank</key><string>None</string>
  <key>LSItemContentTypes</key><array><string>public.folder</string></array>
</dict></array>
<key>CFBundleIconFile</key><string>Crawler</string>
</dict></plist>
PLIST
cp Resources/Crawler.icns "$STAGE/Contents/Resources/Crawler.icns"
cp LICENSE THIRD_PARTY_NOTICES.md "$STAGE/Contents/Resources/"
cp Resources/Fonts/LICENSE.txt "$STAGE/Contents/Resources/IBMPlexMono-LICENSE.txt"
python3 scripts/rust-notices.py crawler-rune "$STAGE/Contents/Resources/RUST-THIRD-PARTY.txt"
codesign --force --sign "$SIGNING_IDENTITY" "$STAGE"
codesign --verify --strict "$STAGE"

if [[ -e "$APP" ]]; then
  rm -rf "$APP.previous"
  mv "$APP" "$APP.previous"
fi
mv "$STAGE" "$APP"
rm -rf "$APP.previous"
printf '%s\n' "$APP"
