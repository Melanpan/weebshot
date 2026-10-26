# Contributing to weebshot

Notes for working on the code. (The user-facing docs live in [`docs/`](docs/) and are built into
the app's Help window.)

## Building

See [Building from source](README.md#building-from-source) in the README. While weebshot is running
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
  (`scripts/package-linux.sh`) and the macOS app (`scripts/package-macos.sh`), then publishes a
  GitHub Release with all three attached and generated release notes.

To release:

1. Bump `version` in `Cargo.toml` and commit.
2. `git tag v0.2.0 && git push origin main v0.2.0`
3. Wait for the *Release* workflow; edit the release notes on GitHub if you like. Running copies of
   weebshot will offer the update within a day (asset names must stay as they are — the updater
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
- `src/player.rs` — libmpv playback and OpenGL rendering into the egui window.
- `src/thumbs.rs`, `src/waveform.rs`, `src/subs.rs` — background decoding of thumbnails, the
  audio waveform and subtitle timings for the timeline.
- `src/keymap.rs`, `src/theme.rs`, `src/fonts.rs`, `src/sounds.rs`, `src/settings.rs`, `src/log.rs`.

## Regenerating the documentation screenshots

`docs/images` is produced by the app itself — it drives its own UI through a few scenes and saves
what it renders (it doesn't capture the screen):

```powershell
cargo run -- --docs-screenshots "C:\videos\episode.mkv" docs/images
```

It needs an episode with an English subtitle track, uses default settings (yours are neither read
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
WEEBSHOT_SAMPLE="path/to/sample.mkv" cargo test -- --ignored --nocapture
```

## Assets

- `assets/fonts` — Phosphor icon font (MIT, see `PHOSPHOR-LICENSE.txt`).
- `assets/sounds` — see `assets/sounds/README.md` for the shutter sample's license.
- `assets/mascot*.png`, `assets/icon.png`, `assets/icon.ico` — the mascot and the round program
  icon cropped from it. `assets/weebshot.rc` embeds the icon into the exe (via `build.rs`).
