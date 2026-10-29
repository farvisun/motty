#!/bin/sh
# Builds a universal (Apple silicon + Intel) Motty.app and wraps it in
# dist/Motty.dmg. The app is signed ad hoc, not notarized: on first launch
# macOS asks users to confirm it (right-click > Open).
set -eu
cd "$(dirname "$0")/.."

case "$(uname -m)" in
  arm64) other=x86_64-apple-darwin ;;
  x86_64) other=aarch64-apple-darwin ;;
  *) echo "unsupported host: $(uname -m)" >&2; exit 1 ;;
esac

# The bundle (icon, Info.plist, AppleScript dictionary) for the host arch,
# then the other arch's binaries merged into it.
cargo xtask bundle --release
cargo build --release --target "$other" -p motty-app -p motty-cli

app=target/release/Motty.app
lipo -create target/release/motty-app "target/$other/release/motty-app" \
  -output "$app/Contents/MacOS/Motty"
lipo -create target/release/motty "target/$other/release/motty" \
  -output "$app/Contents/Resources/bin/motty"
codesign --force --sign - "$app/Contents/Resources/bin/motty"
codesign --force --sign - "$app"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"

mkdir -p dist
rm -f dist/Motty.dmg
hdiutil create -quiet -volname "Motty $version" -srcfolder "$stage" -fs HFS+ -format UDZO dist/Motty.dmg
cp dist/Motty.dmg "dist/Motty-$version-universal.dmg"
shasum -a 256 dist/*.dmg
