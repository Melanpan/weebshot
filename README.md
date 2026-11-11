# Weebshot

<img src="assets/mascot.png" alt="Weebshot mascot: a girl in a director's chair holding a clapperboard" align="right" width="230">

A screenshot and clip tool for anime. Open an episode, find the frame or line you want, and save a
full-resolution screenshot or cut a WebM, MP4, MKV, GIF or animated WebP, with subtitles burned in,
left out, or both.

[![Release](https://img.shields.io/github/v/release/Melanpan/weebshot?sort=semver)](https://github.com/Melanpan/weebshot/releases/latest)
[![CI](https://github.com/Melanpan/weebshot/actions/workflows/ci.yml/badge.svg)](https://github.com/Melanpan/weebshot/actions/workflows/ci.yml)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS%20%28Apple%20Silicon%29-lightgrey)

> [!WARNING]
> **This project was vibecoded** — written largely by an AI coding assistant, with a human steering
> and testing. It works and has a decent test suite, but it hasn't had the scrutiny of a carefully
> hand-written project. Expect rough edges, and please report what you find. (Weebshot only ever
> *creates* new files; it never modifies your videos.)

![Weebshot with an episode open: In/Out set on a subtitle line, thumbnails, waveform and subtitle lane on the timeline](docs/images/hero.jpg)

## What it makes

The bundled demo scene is available as both a WebM clip and a GIF:

<img src="docs/examples/gekota-kick.gif" alt="Example GIF: 'Take this! Heroic Gekota Kick!' with burned-in subtitles" width="480">

| | Settings | Size |
| --- | --- | --- |
| [**WebM**](docs/examples/gekota-kick.webm) (click to play) | 1080p, VP9 + Opus, subtitles burned in, *fit into 8 MB* | 3.7 MB · 6.1 s |
| **GIF** (above) | 360p, 15 fps, Balanced preset, duplicate frames dropped | 2.6 MB · 6.1 s |

## Download

| Platform | Download | |
| --- | --- | --- |
| **Windows** 10/11 (x64) | [`weebshot-windows-x64.zip`](https://github.com/Melanpan/weebshot/releases/latest/download/weebshot-windows-x64.zip) | unzip and run `weebshot.exe` |
| **Linux** (x86-64) | [`weebshot-x86_64.AppImage`](https://github.com/Melanpan/weebshot/releases/latest/download/weebshot-x86_64.AppImage) | make executable and run |
| **Fedora** (x86-64) | [`weebshot-x86_64.rpm`](https://github.com/Melanpan/weebshot/releases/latest/download/weebshot-x86_64.rpm) | `sudo dnf install ./weebshot-x86_64.rpm` |
| **macOS** (Apple Silicon) | [`weebshot-macos-arm64.zip`](https://github.com/Melanpan/weebshot/releases/latest/download/weebshot-macos-arm64.zip) | right-click → Open the first time · *experimental* |

Everything is included (player, ffmpeg, gifsicle, Anime4KCPP helper) — no separate installs. Weebshot checks for
updates once a day and can update itself on Windows and Linux AppImage builds. Fedora RPM
installations are upgraded with `dnf install ./weebshot-x86_64.rpm`.
More in [Installing & updating](docs/install.md).

## Features

### Finding a frame
- Arrow keys step one frame at a time (Shift/Ctrl for bigger, configurable steps), clicks snap to
  frames, and you can type a timecode (`00:01:23:12`) or frame number (`#2000`).
- Timeline with a thumbnail filmstrip (every frame, once you zoom in), an audio waveform, subtitle
  lines as blocks, and chapter markers.
- A shuttle control, plus J/K/L, for scanning forward and backward at variable speed.
- ↑/↓ jump between subtitle lines.

![Timeline zoomed in to single frames](docs/images/timeline-frames.jpg)

### Subtitles
- Mark a clip to exactly one subtitle line with a single key press, or a double-click on the
  timeline.
- Subtitle lines can be edited, deleted and retimed directly on the timeline (right-click a line,
  or drag it to retime). Edits are reflected in the preview and in exports.
- Styled ASS subtitles keep their fonts — whatever's embedded in the MKV is used, with the same
  renderer as the preview. PGS (Blu-ray) subtitles work too.
- Exports can include subtitles, leave them out, or do both in one go (`…_subbed` / `…_raw`).

### Screenshots

- Taken by the player itself, at full source resolution (not the window size), as PNG, JPG or
  WebP. `S` uses your current subtitle setting, `Ctrl+S` gives a clean frame.
- **Copy** (`Ctrl+Shift+C`) copies the screenshot image. A gentle ding confirms a saved screenshot.

### Auto Stitch

Reconstruct illustrations from horizontal, vertical, or small diagonal camera pans.
Use the **Stitch** sidebar tab for **Auto Stitch** (`Shift+S`) around the playhead,
or mark In/Out and choose **Stitch Selection** (`Alt+Shift+S`). Preview, zoom, pan,
and save the native-resolution result as PNG. See [Auto Stitch](docs/stitching.md)
for settings, resource limits, and supported scenes. In/Out marks override automatic
discovery; clear them to search around the playhead.

![Auto Stitch preview with four selected frames from an upward camera pan and the reconstructed image](docs/images/auto-stitch.jpg)

### Clips

- WebM (VP9/Opus), MP4 (H.264/AAC), MKV, GIF, animated WebP.
- MP4 and MKV offer **Encoding → Copy** to preserve the source video and selected audio.
  Cuts follow source keyframes, subtitles are omitted, and the source codecs must fit the container.
  See [Copy without re-encoding](docs/clips.md#copy-without-re-encoding).
- **Copy** (`Shift+E`) exports the clip and copies the finished files to the clipboard.
- Can target a file size (4/8/10/25/50 MB, e.g. for imageboards or Discord) with a 2-pass encode
  that lands just under it.
- Can also repeat a clip's content a set number of times, for making loop videos.
- GPU encoding via NVENC, Quick Sync or AMF, where available — found by actually testing each one
  rather than just checking for drivers.
- Re-encoding converts 10-bit sources for broad playback compatibility and makes frame-accurate cuts.

### GIFs

- Per-clip optimal palette, duplicate-frame dropping (anime is often drawn on 2s/3s),
  changed-area-only encoding, adjustable colors and dithering, and optional gifsicle lossy
  compression, with Small/Balanced/High presets.

### Other things

- Resizable sidebar with **Capture**, **Stitch**, **Media**, **Queue**, and **Logs** tabs.
  The selected tab is remembered, and Capture shares its output folder with Stitch.
- Export queue runs in the background, with progress, retry, and a log of every ffmpeg command run.
- Rebindable shortcuts, a few themes (Gruvbox, Nord, Catppuccin, Dracula), UI scaling.
- Built-in documentation (F1) that reflects your current key bindings.
- A short sound when a screenshot or export finishes.

<details>
<summary>More screenshots</summary>

![GIF settings](docs/images/gif-options.jpg) ![Export queue](docs/images/queue.jpg)

![Options](docs/images/options.jpg)

![Gruvbox theme](docs/images/theme-gruvbox.jpg)

![Earlier Weebshot interface on macOS](docs/images/macos.jpg)

The macOS screenshot shows an earlier interface.

</details>

## Documentation

The same pages are built into the app (*Help → Documentation*, or F1):

[Getting started](docs/getting-started.md) ·
[Installing & updating](docs/install.md) ·
[The interface](docs/interface.md) ·
[Timeline & seeking](docs/timeline.md) ·
[Screenshots](docs/screenshots.md) ·
[Auto Stitch](docs/stitching.md) ·
[Clips](docs/clips.md) ·
[GIFs](docs/gifs.md) ·
[Keyboard shortcuts](docs/shortcuts.md) ·
[Options & themes](docs/options.md) ·
[Troubleshooting](docs/troubleshooting.md)

For implementation details, see the [technical documentation](docs/technical/README.md).

## Building from source

You need [Rust](https://rustup.rs) (stable). At runtime Weebshot looks for `ffmpeg` (and optionally
`gifsicle`) next to itself, then on your `PATH`.

**Anime4K upscaling also needs the `weebshot-anime4k` helper**, which `cargo build` does
not build. It is optional for other features. Building it requires CMake 3.28+, a C++17
compiler, FFmpeg development libraries, and OpenCL development headers and libraries
for GPU support. See [Building the Anime4K helper](CONTRIBUTING.md#building-the-anime4k-helper)
for commands and a CPU-only option. Cargo launches find it in `target/anime4k`
(or `target/anime4k/Release` on Windows); installed builds look next to the app, then
on `PATH`. The platform packaging scripts build and bundle it automatically.

**Windows** — with the Visual Studio Build Tools (C++) installed:

```powershell
./scripts/fetch-libmpv.ps1        # downloads libmpv and creates its import library
cargo run --release
./scripts/package.ps1             # optional: zip with ffmpeg, gifsicle and Anime4K helper in dist/
```

**Linux** (Ubuntu 24.04 or newer, for libmpv 0.37+):

```sh
sudo apt install libmpv-dev ffmpeg gifsicle libxkbcommon-dev libgl1-mesa-dev libwayland-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev
cargo run --release
./scripts/package-linux.sh        # optional: AppImage in dist/
```

**macOS** (Apple Silicon only):

```sh
brew install mpv ffmpeg gifsicle
LIBRARY_PATH="$(brew --prefix)/lib" cargo run --release
```

Pushing a `v*` tag builds all three platforms on GitHub Actions and publishes a release. See
[CONTRIBUTING.md](CONTRIBUTING.md) for how the code is organised and how to run the tests.

## License

Weebshot is free software under the [GNU General Public License v3.0 or later](LICENSE).

It's built on the shoulders of [mpv](https://mpv.io), [FFmpeg](https://ffmpeg.org),
[gifsicle](https://www.lcdf.org/gifsicle/), [egui](https://github.com/emilk/egui) and
[Phosphor icons](https://phosphoricons.com); the release packages include mpv, FFmpeg, gifsicle and Anime4KCPP
under their own licenses — see [THIRD-PARTY.md](THIRD-PARTY.md).
