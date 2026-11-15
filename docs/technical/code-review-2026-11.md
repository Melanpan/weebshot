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

Ordered roughly by impact. The first two change export output.

### 1. Cropped exports get upscaled

[`src/export/ffmpeg.rs:183`](../../src/export/ffmpeg.rs)

`video_chain` only adds a `scale` filter when `max_height` is below the source height, so
that a clip is never upscaled. Without Anime4K, the height it compares against is
`src_height`, ignoring the crop. A 540 px crop from a 1080p source with a 720p limit
passes the check (720 < 1080) and is scaled up to 720. GIF and WebP heights are affected
the same way.

The Anime4K branch already uses the crop height. Fix: use
`self.crop.map(|c| c.height).or(self.src_height)` in both branches, and extend the
`no_upscale` test with a crop.

### 2. Editing multiline subtitles flattens line breaks

[`src/subs.rs`](../../src/subs.rs)

The inline editor works on the stripped display text, where `\N` has become a
space. Editing a two-line cue flattens it to one line, in addition to the documented loss
of override tags on edited lines.

### 3. Timecode display and parsing disagree at 23.976 fps

[`src/util.rs:33`](../../src/util.rs) and `:64`

`fmt_timecode` converts the time to a frame number and divides by the rounded rate (24),
which is non-drop-frame timecode. `parse_position` reads `HH:MM:SS:FF` as real seconds
plus `FF / 24`. The two are not inverses for fractional rates: at 3600 s the display reads
`00:59:56:10`, and typing that back seeks to 3596.4 s, 3.6 s early.

Fix: in the four-field case, compute the frame number
(`((h * 3600 + m * 60 + s) * round(fps) + f)`) and divide by the real `fps`. Add a
round-trip test at 24000/1001.

### 4. A pending exact seek can be downgraded

[`src/player.rs:602`](../../src/player.rs)

The comment says a precise request must not be replaced by a later keyframe seek to the
same position. The code overwrites `pending_seek` unconditionally. Either implement the
rule (keep `exact` when the target matches the pending one) or remove the comment.

Two neighbouring gaps in the same queue: `seek_relative` and single-frame stepping send
their command directly, so a queued `pending_seek` dispatched afterwards overrides them.

### 5. Files mpv cannot open fail silently

[`src/player.rs:306`](../../src/player.rs), [`src/app/mod.rs:327`](../../src/app/mod.rs)

`loadfile` succeeds as soon as the command is accepted. `MPV_EVENT_END_FILE` is never
handled, so a corrupt or non-video file becomes the current file, is pushed to the recent
list, and shows a blank viewer with no message.

Fix: handle `END_FILE` with reason `error` in `poll()`, surface it through `App::error`,
and only call `push_recent` after `FILE_LOADED`.

### 6. `open()` does not stop the shuttle or play-a-line

[`src/app/mod.rs:327`](../../src/app/mod.rs)

`close()` calls `stop_shuttle()` and clears `play_until`; `open()` does neither. Opening a
file while reverse-shuttling keeps issuing seeks from the previous file's `reverse_pos`,
and a pending play-a-line stop time carries over.

### 7. The waveform can desync on an odd-length read

[`src/waveform.rs:70`](../../src/waveform.rs)

The decoder reads 16-bit samples from a pipe and processes `buf[..n - n % 2]`. If a read
returns an odd byte count, the last byte is dropped rather than carried, and every later
sample is shifted by one byte into noise. Pipe reads normally return even counts on Linux,
so this is rare there and more plausible on Windows.

Fix: keep the odd byte and prepend it to the next read.

### 8. Quitting mid-export leaves staging directories behind

[`src/app/mod.rs:1623`](../../src/app/mod.rs), [`src/export/mod.rs:700`](../../src/export/mod.rs)

`on_exit` cancels a running stitch and waits for it, but does nothing for running exports.
The worker thread dies with the process, `Scratch::drop` never runs, and the hidden
`.weebshot-<pid>-<id>-<n>` folder next to the output stays. With Anime4K it holds the
FFV1 intermediates, which run to gigabytes.

Fix: set every active job's cancel flag in `on_exit` and give the runner the same short
grace period the stitch gets. A startup sweep that removes `.weebshot-*` folders whose PID
is no longer running would also cover crashes and kills.

### 9. Windows self-update likely fails for non-ASCII user names

[`src/update.rs:279`](../../src/update.rs)

The helper `.cmd` is written as UTF-8 and contains paths under `%TEMP%`, which is inside
the user profile. `cmd.exe` reads batch files in the OEM code page, so a profile path with
non-ASCII characters is mangled, `robocopy` fails, and the old version restarts. The
robocopy exit code is not checked either.

Fix: start the script with `chcp 65001 >nul`. This one needs testing on Windows.

Separately, `install()` (`:230`) uses the fixed name `weebshot-update` in the shared temp
directory, removing and recreating it. On a multi-user machine another user can pre-create
or symlink that path. `export::Scratch` already creates unique 0700 directories and could
be reused.

### 10. Smaller issues

- **Dropped subtitle files are opened as video.** `handle_input`
  ([`src/app/mod.rs:1317`](../../src/app/mod.rs)) passes any dropped file to `open()`. A
  file with a `SUBTITLE_EXTENSIONS` extension should go to `player.add_sub` instead.
- **Stale thumbnail request.** `Thumbs::request`
  ([`src/thumbs.rs:256`](../../src/thumbs.rs)) returns early when the view is wider than
  `MAX_DETAIL_SPAN` but leaves an earlier `want` in place, so a detail decode still starts
  150 ms later for a range that is no longer shown. Clear `want` on that path.
- **Preview subtitle file is never deleted.** `sync_preview_subs`
  ([`src/app/mod.rs:504`](../../src/app/mod.rs)) writes
  `weebshot-<pid>-preview.subs.ass` to the temp directory and leaves it there on exit. The
  name is also predictable and `File::create` follows symlinks.
- **Output-exists check happens after the encode.** `run_job`
  ([`src/export/mod.rs:720`](../../src/export/mod.rs)) only refuses an existing output once
  the whole encode has finished. The GUI picks a free name up front, so this mainly
  affects explicit paths from MCP; checking before the first step as well saves the work.

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

## Suggested order

1. Bug 1, because it silently changes the resolution of exported files.
2. Bugs 5, 6 and 8, which are small, contained fixes.
3. Optimization 1, then 2.
4. The rest as time allows; bug 9 needs a Windows machine.
