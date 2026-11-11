#!/usr/bin/env bash
# Run inside a disposable Fedora container as root. Installs and removes the RPM.
set -euo pipefail
package="$(realpath "${1:?Usage: test-rpm.sh /path/to/weebshot-x86_64.rpm}")"
work="$(mktemp -d)"
xvfb_pid=
cleanup() {
    [[ -z "$xvfb_pid" ]] || kill "$xvfb_pid" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

dnf install -y "$package" xorg-x11-server-Xvfb shadow-utils util-linux
useradd --create-home --shell /bin/bash weebshot-test
chown weebshot-test:weebshot-test "$work"
user=(runuser -u weebshot-test --)
export TMPDIR="$work"
rpm -V weebshot
rpm -q --qf '[%{FILEMODES:perms} %{FILEUSERNAME} %{FILEGROUPNAME} %{FILENAMES}\n]' weebshot > "$work/permissions.txt"
awk '
    $2 != "root" || $3 != "root" { print "Non-root package ownership:", $0; bad=1 }
    substr($1,1,1) == "d" && $1 != "drwxr-xr-x" { print "Inaccessible package directory:", $0; bad=1 }
    substr($1,1,1) == "-" && $1 != "-rw-r--r--" && $1 != "-rwxr-xr-x" { print "Unsafe package permissions:", $0; bad=1 }
    END { exit bad }
' "$work/permissions.txt"
runtime=/usr/lib64/weebshot/usr/bin
"${user[@]}" test -x /usr/bin/weebshot
test -f /usr/share/applications/weebshot.desktop
test -f /usr/share/icons/hicolor/256x256/apps/weebshot.png
"${user[@]}" "$runtime/weebshot-anime4k" probe cpu
"${user[@]}" "$runtime/gifsicle" --version
"${user[@]}" "$runtime/ffmpeg" -hide_banner -encoders > "$work/encoders.txt" 2>/dev/null
grep -Fq libx264 "$work/encoders.txt"
grep -Fq libvpx-vp9 "$work/encoders.txt"
"${user[@]}" "$runtime/ffmpeg" -y -v error -f lavfi -i 'testsrc2=s=64x48:r=1:d=1' -frames:v 1 "$work/source.png"
"${user[@]}" "$runtime/weebshot-anime4k" image cpu acnet-f8b4 "$work/source.png" "$work/upscaled.png"
# PNG IHDR stores width/height as big-endian integers: 128 x 96.
test "$(od -An -t u1 -j16 -N8 "$work/upscaled.png" | xargs)" = '0 0 0 128 0 0 0 96'
"${user[@]}" "$runtime/ffmpeg" -v error -i "$work/upscaled.png" -f null -

# Exercise the installed launcher, desktop graphics dependencies, playback and
# GUI exports on a software-rendered X server, without a host display or GPU.
printf '1\n00:00:00,500 --> 00:00:04,500\nFedora RPM playback and export validation\n' > "$work/sample.srt"
"${user[@]}" "$runtime/ffmpeg" -y -v error -f lavfi -i 'testsrc2=s=640x360:r=24:d=6' \
    -i "$work/sample.srt" -f lavfi -i 'sine=f=440:r=48000:d=6' \
    -map 0:v -map 1:s -map 2:a -c:v libx264 -preset ultrafast -c:s srt -c:a aac "$work/sample.mkv"
Xvfb :99 -screen 0 1600x1000x24 -nolisten tcp -ac > "$work/xvfb.log" 2>&1 &
xvfb_pid=$!
for _ in {1..30}; do
    [[ ! -S /tmp/.X11-unix/X99 ]] || break
    sleep 0.1
done
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1
install -d -o weebshot-test -g weebshot-test "$work/shots"
if ! timeout 120 "${user[@]}" /usr/bin/weebshot --docs-screenshots "$work/sample.mkv" "$work/shots" --at 2 > "$work/gui.log" 2>&1; then
    cat "$work/xvfb.log" "$work/gui.log"
    exit 1
fi
shopt -s nullglob
shots=("$work/shots/"*.jpg)
if [[ "${#shots[@]}" != 10 ]]; then
    echo "Expected ten GUI captures, found ${#shots[@]}"
    cat "$work/gui.log"
    exit 1
fi
exports=("${TMPDIR:-/tmp}/weebshot-docs-exports/"*.gif "${TMPDIR:-/tmp}/weebshot-docs-exports/"*.webm)
if [[ "${#exports[@]}" != 2 ]]; then
    echo "Expected GIF and WebM exports, found ${#exports[@]}"
    cat "$work/gui.log"
    exit 1
fi
for file in "${exports[@]}"; do
    test -s "$file"
    "${user[@]}" "$runtime/ffmpeg" -v error -i "$file" -f null -
done
if [[ -n "${RPM_TEST_OUTPUT:-}" ]]; then
    mkdir -p "$RPM_TEST_OUTPUT"
    cp "${shots[@]}" "$work/gui.log" "$RPM_TEST_OUTPUT/"
fi
echo 'Installed launcher rendered all ten GUI scenes and exported valid GIF/WebM clips.'

# A private runtime must not advertise bundled SONAMEs to unrelated RPMs.
if rpm -q --provides weebshot | grep -Eq '^lib.*[.]so'; then
    echo 'Private bundled libraries were advertised as global RPM providers.' >&2
    exit 1
fi
dnf remove -y weebshot
test ! -e /usr/bin/weebshot
test ! -e /usr/lib64/weebshot
test ! -e /usr/share/applications/weebshot.desktop
test ! -e /usr/share/icons/hicolor/256x256/apps/weebshot.png
test ! -e /usr/share/licenses/weebshot
test ! -e /usr/share/doc/weebshot
echo 'Fedora RPM install, runtime, upscaling, GUI exports, and removal passed.'
