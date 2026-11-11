# Changelog

## Unreleased

## 0.4.0 — 2026-11-11

### New

- **Anime4K 2× upscaling** for screenshots, clipboard images, and MP4, MKV, WebM, GIF,
  and animated WebP clips. Cropping and subtitles are applied before upscaling; clip height
  limits are applied afterwards. Screenshot and clip choices are saved separately and start off.
- **ACNet and ARNet models**, each with Fast, Balanced, and Slow presets, plus Neutral,
  Denoise, Line restoration, and Line restoration + denoise variants. ACNet Balanced is the default.
- **Auto, GPU, and CPU upscaling** through the bundled Anime4KCPP helper. Auto tries a
  compatible OpenCL GPU and reports CPU fallback in Queue and Logs; CPU needs no GPU driver.
  Upscaling is independent of the hardware video encoder. Jobs support progress, cancellation,
  and retry, while preserving clip timing, audio selection, looping, and file-size targets.
- **Auto Stitch** reconstructs artwork revealed by horizontal, vertical, and small diagonal
  camera pans. Use **Shift+S** to discover the pan around the playhead, or mark In/Out and use
  **Stitch Selection** (**Alt+Shift+S**). With both marks set, either action uses that exact range.
  Actions are available in the Stitch tab, Export menu, and video/timeline context menus.
- **Stitch methods and controls**: Automatic, Single translation, Two vertical layers with
  an optional manual divider, Shift and scale for pans with a uniform zoom, and experimental
  Layered motion with foreground/background priorities. Frame spacing, analysis rate, search
  window, and short-pause handling are configurable.
- **Full-resolution stitch preview and PNG export**, with zoom/pan, Fit, output dimensions,
  confidence, camera direction, actual time range, and expandable warnings about omitted frames,
  search limits, and layer joins. Non-square pixel aspect ratios are retained in PNG metadata.
  Stitch jobs share the background export worker and support progress, cancel, and retry.
- **Choose stitch source frames** using thumbnails and include/exclude controls. Apply the
  selection to recompose in the background using the existing alignment, or select all, deselect
  all, and revert changes. A failed or cancelled rebuild preserves the previous preview.
- **Photoshop PSD export** with aligned source frames on separate layers, timestamped layer
  names, editable seam masks, and a merged image matching the PNG. Two-layer stitches export
  each side separately; oversized PSD requests report the memory limit.
- **A reorganised sidebar** with Capture, Stitch, Media, Queue, and Logs tabs. Capture separates
  Screenshot and Clip settings; Media groups track selection and file information. The selected
  tab is restored at launch, and Capture and Stitch share the output folder.
- **Copy images and exported files to the clipboard**: screenshot images (**Ctrl+Shift+C**),
  completed clip files (**Shift+E**), and full-resolution stitch images (**Alt+C**), with buttons
  and rebindable shortcuts. Screenshot copying follows crop, subtitle, and Anime4K settings;
  Both copies the subbed image. Clip copying waits for every requested variant to finish and
  leaves the clipboard unchanged if any export fails or is cancelled.
- **MP4/MKV stream copy**: choose **Encoding → Copy** to preserve the original compressed
  video and selected audio without re-encoding. Cuts start at the preceding keyframe, source
  codecs must fit the container, and subtitles are omitted. The mode is remembered; switching
  back to Re-encode restores the existing processing settings.

### Improved

- Capture shortcuts stay visible in narrow sidebars. Playback controls adapt to two rows and
  horizontal scrolling when space is limited, keeping buttons from overlapping or disappearing.
- Stitch alignment tolerates limited foreground animation, rejects unreliable poses using
  line-art agreement, and places overlap seams away from moving regions where possible.
  Translation-only stitching preserves individual source pixels instead of averaging overlaps.
- Stitch analysis uses adaptive sampling, bounded searches, scene/motion rejection, and
  nonadjacent alignment checks. Preview details explain the retained stable sequence and limits.
- Cached alignment reference pyramids and reduced repeated pixel scoring speed up stitching.
  PNG export writes pixel-aspect metadata in one pass instead of copying large images twice.
- Release builds show info, warnings, and errors in Logs by default; debug messages remain
  available through the level filter. Logs cache filtered rows, and Queue reads completed file
  sizes once instead of on every repaint.
- Saved screenshots now use a gentle single ding instead of the shutter sound. Screenshot
  format labels use **JPEG** consistently.
- Refreshed built-in help and README screenshots explain the sidebar, clipboard actions,
  Auto Stitch, stream copy, upscaling models/devices, and source-build requirements.

### Fixed

- **Backward scrubbing and screenshots with hardware decoding**: use CPU-readable decoder
  output and avoid Vulkan Video copy paths that could hang on NVIDIA during backward seeks.
  Pause and hardware-decoding changes are asynchronous, and seek completion uses observed
  positions instead of blocking the UI on synchronous decoder queries. Logs identify the decoder.
- **Stale file state**: opening a video resets duration, dimensions, codec, frame rate, position,
  and seek state before the new metadata arrives, preventing thumbnails from using the previous
  file's aspect ratio or frame grid.
- **Closing a video** now stops shuttle/range playback and releases its thumbnail and waveform
  workers, caches, and timeline state. Thumbnail eviction also works in the keyframe overview
  of long files. Completed jobs release captured screenshot and retry data they no longer need.
- **Loop export memory use**: MP4, MKV, and WebM encode once and repeat the encoded packets,
  rather than retaining full-resolution decoded frames in RAM. GIF/WebP loop buffers are sized
  to the actual animation frame count.
- **Temporary-file usage and cleanup**: large encode intermediates, two-pass logs, palettes,
  and Anime4K's lossless FFV1 videos are staged beside the destination instead of filling a
  RAM-backed system temporary directory. Per-job scratch directories are cleaned up on success,
  failure, and cancellation. Unix scratch directories use private permissions.
- **Edited subtitle exports** snapshot the current ASS data for the queued job and materialise
  it in that job's scratch directory, avoiding persistent temporary subtitle files.
- **Output collisions and partial files**: queued outputs reserve their filenames, so repeated
  exports cannot choose the same name before the first job finishes. Clip exports are published
  only after successful completion; cancellation does not leave a partially encoded final file.
  Stitch/image saving uses sibling staging files and rejects the open source video as a destination.
- **Subtitle timeline interaction** no longer gets stuck when the track changes or a cue is
  deleted during an inline edit; stale editing and selection state are cleared or adjusted.
- **Cropped screenshots** avoid unsigned coordinate underflow at preview edges. FFmpeg-rendered
  PNG screenshots are explicitly 8-bit, matching player screenshots even with 10-bit sources.
- **Stitch previews** no longer remain collapsed to a title bar after restoring older window state.
- **Stitch range decoding** skips unused preroll and stops after the last selected frame instead
  of continuing toward the end of the episode. Decoder timeouts measure inactivity, allowing
  slow decodes to continue while they are making progress.
- **Stitch alignment drift and early animated poses** no longer force unreliable frames into
  the result. Previews report omitted frames and their actual covered range, and explicit ranges
  containing a scene cut are rejected rather than silently shortened.
- **Update checks and downloads** now have connection and transfer timeouts so a stalled
  request cannot leave the updater busy indefinitely.
- **Tool redetection** waits until exports and stitches have finished before replacing their worker.
  Anime4K two-pass analysis and GIF processing avoid opening an unused source audio input.
- **Windows build checks** no longer fail on Unix-only scratch-directory mutability warnings.

### Builds and verification

- Windows, Linux AppImage, and macOS Apple Silicon packaging now build and bundle the
  Anime4KCPP helper, its required runtime libraries, and dependency licence notices, and probe
  the packaged CPU backend. Source builds document the separate CMake helper build and a
  CPU-only option; plain `cargo build` still builds the Rust app alone.
- Release workflows run the test suite on all three platforms before packaging. Linux CI installs
  FFmpeg so generated-media tests execute rather than skip.
- Added synthetic stitch reconstruction and real-FFmpeg regression fixtures for translation,
  layered motion, zoom, composition, PNG aspect metadata, frame selection, and export safety.
  Added stream-copy packet checks, settings/output-name regressions, a hardware-decoder
  backward-seek/screenshot regression, and Anime4K format/timing/subtitle/cancellation coverage.
- Validated the Windows package in a Windows 11 VM: 92 regular tests and both optional Anime4K
  integration tests passed, alongside Clippy and formatting. CPU/GPU/Auto upscaling, paths with
  Japanese characters/spaces/apostrophes, GUI playback, and GIF/WebM exports passed. Real anime
  clips and screenshots also passed 2× upscaling, frame/timestamp checks, and MP4/WebM/GIF
  encoding/decoding, including 1080p-to-4K output.

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
