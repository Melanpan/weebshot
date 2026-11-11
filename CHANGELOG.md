# Changelog

## Unreleased

- Replaced Inspector with Capture, Stitch, Media, Queue, and Logs tabs. Capture separates
  Screenshot and Clip settings. The selected tab is restored at launch.
- Added dedicated Auto Stitch and Stitch Selection sidebar actions using the shared timeline range.
- Added clipboard buttons and rebindable shortcuts for screenshot images, exported clip files,
  and full-resolution stitch results.
- Added MP4/MKV Copy mode to preserve compressed video and selected audio without re-encoding,
  with keyframe cuts and source-codec compatibility requirements.
- Kept capture shortcuts visible at narrow sidebar widths and made playback controls use
  two rows with scrolling when space is limited.
- Replaced the screenshot shutter sound with a gentle ding.

- Added a stitch fallback for two side-by-side regions panning vertically at
  different speeds, with a preview warning to inspect the divider.
- Added stitch method selection, adjustable frame spacing, and a manual divider
  for two-layer stitches.
- Added Shift and scale for shots where a single artwork plane pans and zooms.
- Added experimental Layered motion for vertical pans with foreground and
  background motion priorities.
- Added Photoshop PSD export with aligned frames and editable seam masks.

- Release builds show info, warnings and errors by default in the Logs tab.
  Debug messages are available through the level filter.

- Added source-frame thumbnails with include/exclude controls and background
  recomposition, reusing alignment and preserving the previous preview on failure.
- Improved pose-change rejection using line-art agreement and replaced fixed
  overlap cuts with seams that avoid moving regions where possible.

- Fixed stitch previews remaining stuck as a title bar when an older collapsed
  window state was restored.

- Refined the stitch preview with a clear dimensions header, confidence badge,
  expandable warning details, grouped zoom controls and a primary Save PNG action.

- Fixed native stitch decoding continuing toward the end of the episode after the
  last selected frame. Seek past unused preroll and stop at the selected frame count.
- Cached alignment reference pyramids and reduced repeated pixel-scoring work.

- Fixed native alignment search drift and recovery from animated poses at the start
  of a pan. Stitch previews now identify omitted frames and their actual time range.

- Added local Auto Stitch for horizontal, vertical and small diagonal camera pans,
  with automatic boundary discovery or explicit In/Out selection.
- Added native-resolution PNG preview/export, zoom and pan, progress, cancellation,
  retry, configurable shortcuts and stitch settings. Jobs share the clip export worker.
- Added bounded Rust translation matching, scene/motion rejection, adaptive sampling,
  nonadjacent alignment checks and source-pixel overlap composition.
- Added deterministic synthetic reconstruction tests and a generated FFmpeg video fixture.

## 0.3.1 — 2026-11-04

### New
- **Load subtitle file…** (*File* menu, **Ctrl+Shift+O**, or the *Load…* button next to the
  Subtitles track in the inspector) lets you pick any subtitle file and have it selected as the
  active track — for files mpv's automatic same-named matching doesn't catch.

## 0.3.0 — 2026-10-28

### New
- **Edit subtitles on the timeline** — right-click a line to edit its text or delete it, or drag
  its body/edges to retime it. Click a line to select it (shown outlined); **Delete** then removes
  the selected line. Edited lines keep their original font, color and position — only the touched
  line's text loses its per-character ASS overrides (bold/italic mid-line, etc). The preview and
  exports burn in the edits.
- **Save subtitles as…** exports the current (possibly edited) subtitle track to a standalone
  `.ass` file.
- **Loop** export option: repeat a clip's content 1–20 times in the output file, for making loop
  videos.
- **Play In→Out** (**Ctrl+Space**) and **Play to Out** (**Shift+Space**) preview the marked range
  without turning on Loop — the first plays the whole range from the top, the second plays from
  wherever you are to the Out point.

### Fixed
- Japanese and other CJK subtitle text now renders in the timeline and the inline editor (a Noto
  Sans JP fallback font is bundled).
- Keyboard shortcuts no longer fire while typing in a text field (e.g. the subtitle editor).

## 0.2.0 — 2026-10-26

### New
- **Match preview (zoom)** — zoom into the preview and export only what you see: an option in
  both the Screenshot and Clip sections (off by default). Burned-in subtitles keep their normal
  size at the bottom of the crop, like in the preview.
- **Preview zoom controls** under the viewer: zoom out / zoom % (with presets) / zoom in, and the
  shortcuts **Ctrl+=**, **Ctrl+−** and **Ctrl+0** (fit). Ctrl+wheel over the picture still zooms at
  the pointer.
- **Dragging In/Out is clearer** — while you drag a marker (or the whole range) the viewer gets a
  yellow frame and a label: *IN POINT* / *OUT POINT* / *MOVING IN → OUT*, with timecode and frame.
- The **title bar shows the open file**.
- **Frame rate cap** for clips: keep the original or limit to 60–12 fps.

### Improved
- Better **default layout**: the timeline fits its lanes exactly, the side panel is wider, and the
  window opens maximized the first time.
- Files now **open paused**, and subtitle timings load first so the subtitle lane appears quickly.
- **Animated WebP** looks better (higher quality, fed RGB) and has its own *Size* setting,
  defaulting to 720p; the GIF size warnings no longer show for WebP.
- It's **Weebshot** with a capital W now, everywhere you read it.
- Docs: new screenshots, example WebM and GIF in the README.

### Fixed
- **macOS**: the app wouldn't start on macOS 15.4 or newer (duplicate library search paths after
  bundling).
- **macOS**: burning in subtitles didn't work — the bundled ffmpeg is now a build with libass.
- **Copy** in the Logs tab and Ctrl/Cmd+C now work (clipboard support was missing).

## 0.1.0 — 2026-10-26

First release: frame-accurate screenshots and WebM/MP4/MKV/GIF/WebP clips with subtitles burned
in, left out or both; timeline with thumbnails, waveform and subtitle lane; shuttle and J/K/L;
fit-into-file-size; GIF optimisation; GPU encoding; export queue; themes; rebindable shortcuts;
built-in docs; self-updating. Windows, Linux (AppImage) and macOS (Apple Silicon) builds.
