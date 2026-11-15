# Code review, November 2026

A read-through of Weebshot 0.4.0 at commit `76a9393`, listing known bugs and possible
optimizations. Use it as a backlog: delete an entry when it is fixed or turns out to be wrong.

**How this was produced.** The player, thumbnails, waveform, subtitles, export, updater,
app core and MCP bridge were read in full; the stitcher was skimmed for hot spots only.
`cargo clippy --all-targets` is clean. Nothing here was reproduced at runtime and the test
suite was not run, so every entry is a finding from reading the code. Confirm each one
before fixing it. Line numbers refer to the commit above.

Not covered: the stitch solver's correctness, the MCP tools and inspection code, the
settings UI, the keymap, and the native Anime4K helper.

## Bugs

All bugs listed in this review have fixes in the source. The native Windows regression
`update::tests::windows_update_copies_restarts_and_handles_locked_files` exercises the
actual updater helper: waiting for the parent, replacing files, restarting, handling a
locked destination, and removing its workspace. Paths contain Japanese characters,
spaces, and an apostrophe. The failure dialog is replaced with a recorder in this test;
a manual end-to-end upgrade from the GUI remains unverified.
Normal exit cancels export jobs and gives workers a two-second cleanup grace period;
forced termination and crashes can still leave staging directories behind.

## Optimizations

### 1. Parallelize stitch motion detection

[`src/stitch/solve.rs:85`](../../src/stitch/solve.rs), [`src/stitch/motion.rs:154`](../../src/stitch/motion.rs)

Motion between each pair of consecutive analysis frames is estimated in a serial loop, and
the stitcher uses no threads anywhere outside its tests. The pairs are independent.
`motion::estimate` also builds a fresh image pyramid for both of its inputs, so every
frame is downscaled twice, once as the later frame of one pair and once as the earlier
frame of the next.

Suggested change: build each frame's pyramid once, then run the pairs under
`std::thread::scope` across the available cores, keeping the cancel check and progress
reporting. This is likely the largest stitch speedup available. The regression benchmark
added in `711223d` can measure it and confirm results are unchanged.

### 2. Add build profiles

`Cargo.toml` defines no profiles. Two cheap additions:

```toml
[profile.dev.package."*"]
opt-level = 2

[profile.release]
lto = "thin"
```

The first makes debug builds of the stitcher and the `image` crate usable; the second is a
small release gain for longer link time.

### 3. Stop polling in the MCP bridge

[`src/mcp/ipc.rs:493`](../../src/mcp/ipc.rs) and `:425`

The accept loop uses a non-blocking listener and sleeps 5 ms per iteration; the two
auxiliary workers wake every 20 ms to check the stop flag. With MCP enabled and nothing
connected, that is about 300 wakeups a second. Blocking on the socket (`poll(2)`, or the
tokio runtime that is already a dependency) and closing the channel to stop the workers
would bring idle wakeups to zero.

### 4. Take screenshots off the UI thread

[`src/player.rs:683`](../../src/player.rs)

`screenshot-to-file` is sent as a synchronous mpv command, so the UI stalls while mpv
encodes the image; PNG at 1080p or above is noticeable. `Player::command_async` already
exists. The change needs a reply id so the result can be reported from `poll()`, and the
sub-visibility toggle must be restored when that reply arrives rather than immediately.

### 5. Cut repeated full-file reads

- **On open.** Three separate ffmpeg processes each read the whole file: subtitle
  extraction, the keyframe thumbnail overview, and the waveform. Thumbnails and the
  waveform also wait for subtitles to finish. A cache keyed by path, size and mtime (cue
  list, waveform peaks, overview thumbnails) would make reopening a file instant, which
  matters most on network shares and hard disks.
- **On export.** Burning in text subtitles passes the source MKV to the `subtitles` filter
  ([`src/export/ffmpeg.rs:53`](../../src/export/ffmpeg.rs)), which demuxes the entire file
  to collect the track. That repeats for every ffmpeg run: twice for two-pass, twice for
  GIF. Extracting the track once into the job's scratch directory would avoid it, but the
  filter currently also picks up fonts attached to the MKV, so attachments would have to
  be dumped and passed with `fontsdir`.

### 6. Minor

- **PNG round trip for single stitch frames.** `stitch::decode::image_frame`
  ([`src/stitch/decode.rs:120`](../../src/stitch/decode.rs)) has ffmpeg encode a PNG that
  is decoded again in-process. Raw `rgb24` avoids both steps, but the pixel aspect ratio
  currently read from the PNG would need another source.
- **Tool detection blocks startup.** `detect_tools`
  ([`src/app/mod.rs:241`](../../src/app/mod.rs)) runs `ffmpeg -version` and
  `gifsicle --version` synchronously before the first frame. Running them on a thread
  shortens startup, mostly on Windows.
- **Thumbnail textures.** Up to 2500 thumbnails are uploaded as individual textures
  ([`src/thumbs.rs:213`](../../src/thumbs.rs)). An atlas would reduce texture count and
  draw calls if the timeline ever shows up in a profile.

## Suggested optimization order

1. Parallelize stitch motion detection.
2. Add build profiles.
3. The remaining optimizations as time allows.
