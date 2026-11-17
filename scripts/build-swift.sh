#!/bin/zsh
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
scripts/bootstrap-video.sh
scripts/bootstrap-fff.sh
swift build -c release --force-resolved-versions
APP="$PWD/build/Crawler-Swift.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp .build/release/Crawler "$APP/Contents/MacOS/Crawler"
# Keep the unstripped executable and dSYM in .build for local symbolication.
xcrun strip -S -x "$APP/Contents/MacOS/Crawler"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Crawler</string>
<key>CFBundleIdentifier</key><string>studio.crawler.native-prototype</string>
<key>CFBundleName</key><string>Crawler</string>
<key>CFBundleDisplayName</key><string>Crawler</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
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
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>CFBundleIconFile</key><string>CrawlerCabinet</string>
</dict></plist>
PLIST
if [[ -f Resources/Crawler.icns ]]; then cp Resources/Crawler.icns "$APP/Contents/Resources/CrawlerCabinet.icns"; fi
cp -R Resources/Fonts "$APP/Contents/Resources/"
cp -R Resources/FileIcons "$APP/Contents/Resources/"
for bundle in .build/release/*.bundle(N); do cp -R "$bundle" "$APP/Contents/Resources/"; done
mkdir -p "$APP/Contents/Frameworks"
VLC_SOURCE="$PWD/.build/video-dependencies/VLCKit - binary package"
ditto "$VLC_SOURCE/VLCKit.xcframework/macos-arm64_x86_64/VLCKit.framework" "$APP/Contents/Frameworks/VLCKit.framework"
cp "$VLC_SOURCE/COPYING.txt" "$APP/Contents/Resources/VLCKit-COPYING.txt"
codesign --force --sign "$SIGNING_IDENTITY" "$APP/Contents/Frameworks/VLCKit.framework"
cp THIRD_PARTY_NOTICES.md "$APP/Contents/Resources/"
cp LICENSE "$APP/Contents/Resources/"
cp .build/fff/THIRD_PARTY.txt "$APP/Contents/Resources/FFF-THIRD-PARTY.txt"
codesign --force --sign "$SIGNING_IDENTITY" "$APP"
codesign --verify --strict "$APP"
printf '%s\n' "$APP"
