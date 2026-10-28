# Changelog

## Unreleased

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
