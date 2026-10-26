#!/usr/bin/env bash
# Builds dist/weebshot-macos-arm64.zip containing weebshot.app (Apple Silicon).
#
# Needs: brew install mpv ffmpeg gifsicle dylibbundler, and Rust.
# The app is not signed or notarized: on first launch, right-click → Open.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
brew_prefix="$(brew --prefix)"
export LIBRARY_PATH="$brew_prefix/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"

cargo build --release
version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"

dist="$root/dist"
app="$dist/weebshot.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp target/release/weebshot "$app/Contents/MacOS/"
cp "$brew_prefix/bin/ffmpeg" "$brew_prefix/bin/gifsicle" "$app/Contents/MacOS/"
cp LICENSE THIRD-PARTY.md "$app/Contents/Resources/"

# Icon: build an .icns from the PNG.
iconset="$dist/weebshot.iconset"
rm -rf "$iconset" && mkdir -p "$iconset"
for size in 16 32 128 256; do
    sips -z $size $size assets/icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) assets/icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/weebshot.icns"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>weebshot</string>
    <key>CFBundleDisplayName</key><string>weebshot</string>
    <key>CFBundleIdentifier</key><string>io.github.melanpan.weebshot</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleExecutable</key><string>weebshot</string>
    <key>CFBundleIconFile</key><string>weebshot</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
</dict>
</plist>
PLIST

# Copy every Homebrew library the three binaries need into Frameworks and rewrite their paths.
for bin in weebshot ffmpeg gifsicle; do
    dylibbundler -of -b -cd \
        -x "$app/Contents/MacOS/$bin" \
        -d "$app/Contents/Frameworks/" \
        -p @executable_path/../Frameworks/
done
# dylibbundler can add the same LC_RPATH more than once, which dyld (macOS 15.4+) refuses to load.
for file in "$app/Contents/MacOS/"* "$app/Contents/Frameworks/"*; do
    for rpath in $(otool -l "$file" | awk '/cmd LC_RPATH/ { getline; getline; print $2 }' | sort | uniq -d); do
        while otool -l "$file" | grep -q "path $rpath "; do
            install_name_tool -delete_rpath "$rpath" "$file"
        done
        install_name_tool -add_rpath "$rpath" "$file"
    done
done
# Rewriting load commands invalidates signatures; ad-hoc sign so Apple Silicon will run it.
codesign --force --deep --sign - "$app"

(cd "$dist" && rm -f weebshot-macos-arm64.zip && ditto -c -k --keepParent weebshot.app weebshot-macos-arm64.zip)
echo "Packaged $dist/weebshot-macos-arm64.zip"
