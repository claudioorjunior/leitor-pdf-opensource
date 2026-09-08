#!/bin/bash
# Empacota o visor nativo como TsuroPDF.app + DMG (assinatura ad-hoc).
# Uso: ./scripts/bundle-macos.sh
# Saída:
#   dist/TsuroPDF.app
#   dist/TsuroPDF-{versão}-aarch64-apple-darwin.dmg
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/TsuroPDF.app"
PDFIUM_RELEASE="chromium/8044"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/crates/tsuro/Cargo.toml" | head -1)"
TRIPLE="aarch64-apple-darwin"
DMG="$ROOT/dist/TsuroPDF-${VERSION}-${TRIPLE}.dmg"

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
# Nomes legados do rename (o build anterior gerava Tsuro.app): nunca reinstalar o app antigo.
# `Tsuro-*.dmg` não casa `TsuroPDF-*.dmg` (após "Tsuro" vem "P", não "-").
# Guardado: sem legado em disco o glob não expande e o `set -e` abortaria o rm.
for legacy in "$ROOT/dist/Tsuro.app" $ROOT/dist/Tsuro-*.dmg; do
  [ -e "$legacy" ] || continue
  rm -rf "$legacy"
done
mkdir -p "$APP/Contents/Resources"
ICON_MASTER="$ROOT/public/tsuro-app-icon-1024.png"
if command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
  ICONSET="$(mktemp -d)/TsuroPDF.iconset"
  mkdir -p "$ICONSET"
  SIZES="16:icon_16x16 32:icon_16x16@2x 32:icon_32x32 64:icon_32x32@2x 128:icon_128x128 256:icon_128x128@2x 256:icon_256x256 512:icon_256x256@2x 512:icon_512x512 1024:icon_512x512@2x"
  for spec in $SIZES; do
    size="${spec%%:*}"
    name="${spec##*:}"
    sips -z "$size" "$size" "$ICON_MASTER" --out "$ICONSET/$name.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/TsuroPDF.icns"
  rm -rf "$ICONSET"
elif python3 -c "import PIL.Image" 2>/dev/null; then
  python3 - "$ICON_MASTER" "$APP/Contents/Resources/TsuroPDF.icns" <<'PY_EOF'
import sys
from PIL import Image
Image.open(sys.argv[1]).save(sys.argv[2])
PY_EOF
else
  echo "aviso: sem sips/iconutil nem PIL — app sem ícone" >&2
fi

# 4. Bundle.
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"
cp "$ROOT/target/release/TsuroPDF" "$APP/Contents/MacOS/TsuroPDF"
cp "$ROOT/libpdfium.dylib" "$APP/Contents/Frameworks/"
cat >"$APP/Contents/Info.plist" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>TsuroPDF</string>
  <key>CFBundleIdentifier</key>
  <string>dev.tsuro.reader</string>
  <key>CFBundleName</key>
  <string>TsuroPDF</string>
  <key>CFBundleDisplayName</key>
  <string>TsuroPDF</string>
  <key>CFBundleIconFile</key>
  <string>TsuroPDF</string>
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
# Entitlement dispensa Library Validation da libpdfium (ad-hoc gera Team IDs
# distintos por binário; sem isto o kernel rejeita a dylib em Frameworks).
codesign -s - --force --deep --options runtime --entitlements "$ROOT/scripts/tsuro.entitlements" "$APP"

# 5. DMG com atalho para /Applications (evita zip → App Translocation).
STAGE="$(mktemp -d)/TsuroPDF"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/TsuroPDF.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "TsuroPDF" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$(dirname "$STAGE")"
echo "Pronto: $APP"
echo "Pronto: $DMG"
