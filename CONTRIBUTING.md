# Contributing to Weebshot

Notes for working on the code. (The user-facing docs live in [`docs/`](docs/) and are built into
the app's Help window.)

## Building

See [Building from source](README.md#building-from-source) in the README. While Weebshot is running
its exe is locked; build into another target directory to keep working:
`$env:CARGO_TARGET_DIR="target/dev"; cargo build`.

The Linux build can be checked from Windows with Docker — the same steps CI runs:

```powershell
docker run --rm -v "${PWD}:/src:ro" ubuntu:24.04 bash -c "apt-get update && apt-get install -y curl build-essential pkg-config libmpv-dev libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev && curl -sSf https://sh.rustup.rs | sh -s -- -y && . ~/.cargo/env && cp -r /src /work && cd /work && cargo test"
```

## Continuous integration and releases

- `.github/workflows/ci.yml` — on every push to `main` and every pull request: `cargo fmt --check`,
  `cargo clippy -- -D warnings` and `cargo test` on Windows and Linux.
- `.github/workflows/release.yml` — on a `v*` tag: builds the Windows zip
  (`scripts/package.ps1`, with pinned ffmpeg and gifsicle), the Linux AppImage
  (`scripts/package-linux.sh`), Fedora RPM (`scripts/package-rpm.sh`), and the macOS app
  (`scripts/package-macos.sh`), then publishes a GitHub Release with all four attached.
  The RPM is installed, exercised, and removed in a clean Fedora container before publication.
- `.github/workflows/rpm.yml` — manually adds an RPM to an existing release using its published,
  checksum-verified AppImage; it runs the same Fedora install/runtime/removal check first.

To release:

1. Bump `version` in `Cargo.toml`, add a `## <version> — <date>` section to `CHANGELOG.md`
   (it becomes the release notes, which the in-app updater shows), and commit.
2. `git tag v0.2.0 && git push origin main v0.2.0`
3. Wait for the *Release* workflow. Running copies of
   Weebshot will offer the update within a day (asset names must stay as they are — the updater
   looks for them).

## How it works

| Part | Uses |
| --- | --- |
| Playback & preview | libmpv, rendered into the window via mpv's OpenGL render API |
| Screenshots | mpv's `screenshot-to-file` (same renderer as the preview) |
| Clips | ffmpeg subprocesses; every command line is shown in the Logs tab |
| Timeline lanes | ffmpeg decodes thumbnails, the audio waveform and subtitle timings in the background |
| UI | egui / eframe; docs rendered with egui_commonmark |

Source layout:

- `src/app/` — the window: menus, viewer + transport, timeline, inspector, queue/logs (`dock.rs`),
  options, docs/about windows, custom widgets, and the docs screenshot script (`shots.rs`).
- `src/export/` — ffmpeg/gifsicle command building, target-size maths, GPU encoder probing, and the
  background job runner.
- `src/stitch/` — translation estimation, pan discovery, bounded FFmpeg decoding, native
  alignment/composition and PNG metadata; no GUI dependencies. `src/app/stitching.rs` owns
  the preview and queues work on the existing export runner.
- `src/player.rs` — libmpv playback and OpenGL rendering into the egui window.
- `src/thumbs.rs`, `src/waveform.rs`, `src/subs.rs` — background decoding of thumbnails, the
  audio waveform and subtitle timings for the timeline.
- `src/keymap.rs`, `src/theme.rs`, `src/fonts.rs`, `src/sounds.rs`, `src/settings.rs`, `src/log.rs`.

See [Frame stitching](docs/technical/frame-stitching.md) for the Auto Stitch pipeline and
its regression fixtures.

## Regenerating the documentation screenshots

`docs/images` is produced by the app itself — it drives its own UI through a few scenes and saves
what it renders (it doesn't capture the screen):

```powershell
cargo run -- --docs-screenshots "C:\videos\episode.mkv" docs/images --at 4:31.5
```

`--at <time>` sets the frame shown and the subtitle line the scenes are built around (default: the
middle of the first readable line about a third in). It needs an episode with an English subtitle track, uses default settings (yours are neither read
nor changed) and writes its test exports to a temp folder. Rebuild afterwards so the in-app help
picks up the new images.

## Tests

```sh
cargo test
```

Unit tests cover ffmpeg command building (escaping, subtitles, GIF pipeline, 2-pass sizing),
timecode parsing, subtitle parsing, the keymap, the timeline zoom maths, and the docs: every
shortcut mentioned in `docs/` must match the default keymap, and `docs/shortcuts.md` must match the
generated table (`UPDATE_DOCS=1 cargo test` regenerates it).

End-to-end encodes through a real ffmpeg:

```sh
# sample: video with audio at stream 2 and an ASS track as the first subtitle stream
WEEBSHOT_SAMPLE="path/to/sample.mkv" cargo test export::tests:: -- --ignored --nocapture
```

Run the stitching tests with `cargo test stitch -- --nocapture`. The
[technical guide](docs/technical/frame-stitching.md#tests) lists the generated FFmpeg
fixtures and optional regressions that use supplied video.

## Assets

- `assets/fonts` — Phosphor icon font (MIT, see `PHOSPHOR-LICENSE.txt`).
- `assets/sounds` — see `assets/sounds/README.md` for the shutter sample's license.
- `assets/mascot*.png`, `assets/icon.png`, `assets/icon.ico` — the mascot and the round program
  icon cropped from it. `assets/weebshot.rc` embeds the icon into the exe (via `build.rs`).

## Building the Anime4K helper

The Rust app can build on its own. To test upscaling from a source checkout, also build the
helper with CMake 3.28+, a C++17 compiler, and FFmpeg development libraries (libavformat,
libavcodec, libavutil). The platform packaging scripts set up the required libraries and
stage the executable, dependencies, and license notices.

On Linux or macOS, configure and build with:

```sh
cmake -S native/anime4k -B target/anime4k
cmake --build target/anime4k --config Release --parallel
target/anime4k/weebshot-anime4k probe cpu
cargo run
cargo test anime4k::tests -- --ignored
```

Source launches through Cargo find the helper in this native build directory or in the
packaging staging directories. Rebuild the helper after changing native code.

Use `probe gpu` to exercise OpenCL on supported hardware. To build a CPU-only helper, pass
`-DAC_CORE_WITH_OPENCL=OFF` at configure time.

## Building the Fedora RPM

Build the Linux AppImage first, then run:

```sh
./scripts/package-rpm.sh
# Or repackage an existing release (must match the supplied version):
./scripts/package-rpm.sh /path/to/weebshot-x86_64.AppImage 0.4.0
```

Requires `rpmbuild`, `rpm`, Python 3.11+, `readelf`, and `desktop-file-validate`.
On Fedora, install `rpm-build`, `binutils`, and `desktop-file-utils`; on Ubuntu,
install `rpm`, `binutils`, and `desktop-file-utils`. The script creates
`dist/weebshot-x86_64.rpm` with versioned RPM metadata. This is an upstream binary
package: it preserves the tested AppImage runtime under `/usr/lib64/weebshot`,
adds a launcher/menu entry/icon, and lets RPM generate requirements for external
libraries while filtering private bundled SONAMEs. It does not need FUSE.

Test installation, media tools, 2x upscaling, installed-launcher GUI exports under Xvfb,
file verification, and removal in a clean Fedora container (the test installs/removes packages, so use a disposable container):

```sh
podman run --rm --security-opt label=disable -v "$PWD:/work:ro" registry.fedoraproject.org/fedora:45 \
  bash /work/scripts/test-rpm.sh /work/dist/weebshot-x86_64.rpm
```
