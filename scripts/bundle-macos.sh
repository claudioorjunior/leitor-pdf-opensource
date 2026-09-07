#!/bin/bash
# Empacota o visor nativo como Tsuro.app + DMG (assinatura ad-hoc).
# Uso: ./scripts/bundle-macos.sh
# Saída:
#   dist/Tsuro.app
#   dist/Tsuro-{versão}-aarch64-apple-darwin.dmg
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/Tsuro.app"
PDFIUM_RELEASE="chromium/8044"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/crates/tsuro/Cargo.toml" | head -1)"
TRIPLE="aarch64-apple-darwin"
DMG="$ROOT/dist/Tsuro-${VERSION}-${TRIPLE}.dmg"

# 1. Pdfium para dev (o engine procura Frameworks, ao lado do binário, ou sistema).
if [ ! -f "$ROOT/libpdfium.dylib" ]; then
  case "$(uname -m)" in
    arm64) ASSET="pdfium-mac-arm64.tgz" ;;
    *) ASSET="pdfium-mac-x64.tgz" ;;
  esac
  echo "Baixando Pdfium ($ASSET)..."
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  curl -sSL --max-time 180 -o "$TMP/pdfium.tgz" \
    "https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE}/${ASSET}"
  tar xzf "$TMP/pdfium.tgz" -C "$TMP"
  cp "$TMP/lib/libpdfium.dylib" "$ROOT/libpdfium.dylib"
  trap - EXIT
fi

# 2. Binário release.
cargo build --release -p tsuro --manifest-path "$ROOT/Cargo.toml"

# 3. Ícone a partir da marca (tsuru).
rm -rf "$APP"
mkdir -p "$APP/Contents/Resources"
if python3 -c "import PIL.Image" 2>/dev/null; then
  python3 - "$ROOT/public/tsuro-mark.png" "$APP/Contents/Resources/Tsuro.icns" <<'PY_EOF'
import sys
from PIL import Image
Image.open(sys.argv[1]).save(sys.argv[2])
PY_EOF
else
  ICONSET="$(mktemp -d)/Tsuro.iconset"
  mkdir -p "$ICONSET"
  SIZES="16:icon_16x16 32:icon_16x16@2x 32:icon_32x32 64:icon_32x32@2x 128:icon_128x128 256:icon_128x128@2x 256:icon_256x256 512:icon_256x256@2x 512:icon_512x512 1024:icon_512x512@2x"
  for spec in $SIZES; do
    size="${spec%%:*}"
    name="${spec##*:}"
    sips -z "$size" "$size" "$ROOT/public/tsuro-mark.png" --out "$ICONSET/$name.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Tsuro.icns"
fi

# 4. Bundle.
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"
cp "$ROOT/target/release/tsuro" "$APP/Contents/MacOS/tsuro"
cp "$ROOT/libpdfium.dylib" "$APP/Contents/Frameworks/"
cat >"$APP/Contents/Info.plist" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>tsuro</string>
  <key>CFBundleIdentifier</key>
  <string>dev.tsuro.reader</string>
  <key>CFBundleName</key>
  <string>Tsuro</string>
  <key>CFBundleDisplayName</key>
  <string>Tsuro</string>
  <key>CFBundleIconFile</key>
  <string>Tsuro</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST_EOF

# Ad-hoc + runtime: sem Apple Developer Program. Gatekeeper pede "Abrir" na 1ª vez.
codesign -s - --force --deep --options runtime "$APP"

# 5. DMG com atalho para /Applications (evita zip → App Translocation).
STAGE="$(mktemp -d)/Tsuro"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/Tsuro.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "Tsuro" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$(dirname "$STAGE")"
echo "Pronto: $APP"
echo "Pronto: $DMG"
