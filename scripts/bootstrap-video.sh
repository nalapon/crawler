#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
DEST="$PWD/.build/video-dependencies"
FRAMEWORK="$DEST/VLCKit - binary package/VLCKit.xcframework/macos-arm64_x86_64/VLCKit.framework"
[[ -d "$FRAMEWORK" ]] && exit 0
mkdir -p "$DEST"
ARCHIVE="$DEST/VLCKit.tar.xz"
curl -fL --retry 2 -o "$ARCHIVE" https://download.videolan.org/pub/cocoapods/prod/VLCKit-3.7.2-3e42ae47-79128878.tar.xz
print '45fc6398c80d1f8dc0e384a9c80704848e9e82a3a382611bf531fa83c198c276  '"$ARCHIVE" | shasum -a 256 -c -
tar -xJf "$ARCHIVE" -C "$DEST"
