#!/usr/bin/env bash
# Builds dist/weebshot-x86_64.AppImage on Ubuntu 24.04 (or newer).
#
# Needs: libmpv-dev ffmpeg gifsicle (apt), Rust, and the GUI build libraries
# (libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev …).
# Also needs CMake 3.28+, libavformat-dev, libavcodec-dev, libavutil-dev,
# ocl-icd-opencl-dev and opencl-clhpp-headers.
set -euo pipefail

LINUXDEPLOY_VERSION="1-alpha-20251107-1"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

cargo build --release

dist="$root/dist"
appdir="$dist/AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/doc/weebshot"
cp target/release/weebshot "$appdir/usr/bin/"
cp LICENSE THIRD-PARTY.md "$appdir/usr/share/doc/weebshot/"
helper_build="$dist/anime4k-build"
helper_stage="$dist/anime4k"
cmake -S native/anime4k -B "$helper_build" -DAC_CORE_WITH_OPENCL=ON
cmake --build "$helper_build" --config Release --parallel
cmake --install "$helper_build" --config Release --prefix "$helper_stage"
cp -r "$helper_stage/licenses" "$appdir/usr/share/doc/weebshot/anime4k"
if [ -r /usr/share/doc/ocl-icd-libopencl1/copyright ]; then
    cp /usr/share/doc/ocl-icd-libopencl1/copyright "$appdir/usr/share/doc/weebshot/anime4k/ocl-icd-copyright"
fi

cp assets/icon.png "$dist/weebshot.png" # linuxdeploy wants it named like the .desktop Icon=

tool="$dist/linuxdeploy-x86_64.AppImage"
if [ ! -x "$tool" ]; then
    curl -fsSL -o "$tool" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/$LINUXDEPLOY_VERSION/linuxdeploy-x86_64.AppImage"
    chmod +x "$tool"
fi

# Libraries linuxdeploy leaves out by default (expected on the host) that a desktop may lack.
find_lib() { ldconfig -p | awk -v lib="$1" '$1 == lib && /x86-64/ { print $NF; exit }'; }

# ffmpeg and gifsicle sit next to weebshot inside the image, where weebshot looks first.
export APPIMAGE_EXTRACT_AND_RUN=1 # CI containers have no FUSE
export LDAI_OUTPUT="$dist/weebshot-x86_64.AppImage"
"$tool" --appdir "$appdir" \
    --executable "$appdir/usr/bin/weebshot" \
    --executable "$(command -v ffmpeg)" \
    --executable "$(command -v gifsicle)" \
    --executable "$helper_stage/bin/weebshot-anime4k" \
    --library "$(find_lib libOpenCL.so.1)" \
    --library "$(find_lib libjack.so.0)" \
    --library "$(find_lib libusb-1.0.so.0)" \
    --library "$(find_lib libfribidi.so.0)" \
    --library "$(find_lib libpipewire-0.3.so.0)" \
    --library "$(find_lib libgpg-error.so.0)" \
    --desktop-file assets/weebshot.desktop \
    --icon-file "$dist/weebshot.png" \
    --output appimage

# Test the helper from the actual image with its bundled runtime libraries.
probe_dir="$(mktemp -d)"
trap 'rm -rf "$probe_dir"' EXIT
(
    cd "$probe_dir"
    "$LDAI_OUTPUT" --appimage-extract >/dev/null
    export LD_LIBRARY_PATH="$probe_dir/squashfs-root/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    ./squashfs-root/usr/bin/weebshot-anime4k probe cpu
)

echo "Packaged $LDAI_OUTPUT"
