#!/usr/bin/env bash
# Build a Fedora-installable binary RPM from the matching, bundled AppImage.
# Usage: scripts/package-rpm.sh [APPIMAGE [VERSION]]
# Needs: rpmbuild, rpm, python3, readelf, desktop-file-validate; no FUSE required.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dist="$root/dist"
appimage="$(realpath "${1:-$dist/weebshot-x86_64.AppImage}")"
version="${2:-$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["package"]["version"])' "$root/Cargo.toml")}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Expected a stable release version (for example 0.4.0), got: $version" >&2
    exit 1
fi
if [[ "$(uname -m)" != x86_64 ]]; then
    echo 'RPM packaging currently supports x86_64 only.' >&2
    exit 1
fi
for tool in rpmbuild rpm python3 readelf desktop-file-validate; do
    command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 1; }
done
[[ -f "$appimage" ]] || { echo "Build the AppImage first: $appimage" >&2; exit 1; }

mkdir -p "$dist/cache"
work="$(mktemp -d "$dist/cache/rpm.XXXXXX")"
trap 'rm -rf "$work"' EXIT
# Execute only the caller-supplied release image's extraction mode, not the app.
chmod +x "$appimage"
(cd "$work" && "$appimage" --appimage-extract >/dev/null)
appdir="$work/squashfs-root"
for file in AppRun usr/bin/weebshot usr/bin/weebshot-anime4k usr/bin/ffmpeg usr/bin/gifsicle; do
    [[ -x "$appdir/$file" ]] || { echo "Missing release payload: $file" >&2; exit 1; }
done
desktop-file-validate "$appdir/weebshot.desktop"

# xkbcommon-x11 is loaded from Fedora at runtime. Keep its core library from the
# same Fedora build too: mixing it with the AppImage's older core can crash when
# the window backend releases keyboard state on exit.
rm -f "$appdir/usr/lib/"libxkbcommon.so* "$appdir/usr/lib/"libxkbcommon-x11.so*

# Private SONAMEs are resolved through the payload's relative RUNPATHs. Do not
# require RPM Fusion/system copies of libraries already shipped inside the RPM,
# or advertise those private libraries as global providers. Keep all external
# ELF requirements (including the actual GLIBC symbol versions) automatic.
filter="$(python3 - "$appdir/usr/lib" <<'PY'
import pathlib, re, subprocess, sys
names=set()
for p in pathlib.Path(sys.argv[1]).rglob('*'):
    if not p.is_file() or p.is_symlink():
        continue
    result=subprocess.run(['readelf','-d',str(p)],capture_output=True,text=True)
    names.update(re.findall(r'\(SONAME\).*\[([^\]]+)\]',result.stdout))
if not names:
    raise SystemExit('No bundled library SONAMEs found')
# SONAMEs in this payload contain letters, digits, dots, dashes and underscores.
if any(not re.fullmatch(r'[A-Za-z0-9_.+-]+',n) for n in names):
    raise SystemExit('Unexpected SONAME in release payload')
escaped=[n.replace('.', '[.]').replace('+', '[+]') for n in sorted(names)]
print('^('+'|'.join(escaped)+')[(].*$')
PY
)"
mkdir -p "$work/tmp"
rpmbuild -bb "$root/packaging/weebshot.spec" \
    --target x86_64 \
    --define "_topdir $work/rpmbuild" \
    --define "_tmppath $work/tmp" \
    --define "weebshot_version $version" \
    --define "appdir $appdir" \
    --define "__requires_exclude $filter" \
    --define '_binary_payload w7.zstdio' \
    --define '_libdir /usr/lib64'

rpm_path="$work/rpmbuild/RPMS/x86_64/weebshot-$version-1.x86_64.rpm"
# Fixed release-asset name; the RPM's own metadata carries its version/release.
cp "$rpm_path" "$dist/weebshot-x86_64.rpm"
# Release RPMs are unsigned; verify both digests without requiring a signing key.
rpm -K --nosignature "$dist/weebshot-x86_64.rpm"
rpm -qp --qf '%{NAME} %{VERSION}-%{RELEASE} %{ARCH}\n' "$dist/weebshot-x86_64.rpm"
echo "Packaged $dist/weebshot-x86_64.rpm"
