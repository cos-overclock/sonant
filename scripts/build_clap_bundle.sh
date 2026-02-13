#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-debug}"

case "$PROFILE" in
  debug)
    ;;
  release)
    ;;
  *)
    echo "Usage: $0 [debug|release]"
    exit 1
    ;;
esac

cd "$ROOT_DIR"
if [[ "$PROFILE" == "release" ]]; then
  cargo build --release
else
  cargo build
fi

LIB_PATH="$ROOT_DIR/target/$PROFILE/libsonant.dylib"
HELPER_PATH="$ROOT_DIR/target/$PROFILE/sonant"
BUNDLE_CONTENTS_DIR="$ROOT_DIR/dist/Sonant.clap/Contents"
BUNDLE_BIN_DIR="$BUNDLE_CONTENTS_DIR/MacOS"
BUNDLE_BIN_PATH="$BUNDLE_BIN_DIR/Sonant"
BUNDLE_HELPER_PATH="$BUNDLE_BIN_DIR/SonantGUIHelper"
BUNDLE_INFO_PLIST="$BUNDLE_CONTENTS_DIR/Info.plist"

if [[ ! -f "$LIB_PATH" ]]; then
  echo "Build output not found: $LIB_PATH"
  exit 1
fi
if [[ ! -f "$HELPER_PATH" ]]; then
  echo "Helper output not found: $HELPER_PATH"
  exit 1
fi

mkdir -p "$BUNDLE_BIN_DIR"
cp "$LIB_PATH" "$BUNDLE_BIN_PATH"
chmod +x "$BUNDLE_BIN_PATH"
cp "$HELPER_PATH" "$BUNDLE_HELPER_PATH"
chmod +x "$BUNDLE_HELPER_PATH"

cat > "$BUNDLE_INFO_PLIST" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundlePackageType</key>
  <string>BNDL</string>
  <key>CFBundleName</key>
  <string>Sonant</string>
  <key>CFBundleIdentifier</key>
  <string>com.sonant.midi_generator</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleExecutable</key>
  <string>Sonant</string>
</dict>
</plist>
EOF

echo "CLAP bundle updated:"
echo "  $BUNDLE_BIN_PATH"
echo "  $BUNDLE_HELPER_PATH"
