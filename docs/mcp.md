# MCP and headless operation

Weebshot's optional [Model Context Protocol](https://modelcontextprotocol.io/) interface lets local automation inspect videos, prepare selections in a running window, extract frames, export clips, and run the existing camera-pan Stitch engine. It needs no model, API key, account, or external service. Normal launches keep MCP disabled by default, including after upgrading existing settings.

## Launch modes

```sh
weebshot                       # normal GUI; integration remains off unless enabled in settings
weebshot --mcp                 # stdio MCP frontend attaching to an existing GUI
weebshot --headless --mcp      # standalone stdio MCP server, without a window or display
```

`--headless` alone prints usage and exits with code 2: a separate CLI export interface is outside this feature's scope. MCP flags are handled before fonts, player, egui, OpenGL, or window initialization. The executable still links the normal application's libraries; headless mode does not remove installation dependencies.

For GUI control, start Weebshot normally and enable **Options (Settings) → Advanced → MCP → Enable MCP integration**. Changes apply immediately; no restart is required. The page shows both launch commands, the socket path, and a button to copy client configuration. MCP controls are confined to this settings page.

The GUI owns its automation jobs. Closing a stdio attachment leaves those jobs running in the window. Disabling integration closes the socket, stops accepting commands, cancels automation jobs, and discards pending preparations. Ordinary manually queued jobs are unaffected. A headless process owns its own context and queue; EOF, Ctrl+C or SIGTERM cancels its unfinished jobs and joins the worker, reaping its subprocesses.

GUI attachment currently supports **Linux and macOS**, using a private Unix socket. Headless stdio also builds on Windows; Windows GUI attachment is not yet implemented and returns an explicit error. Native Windows/macOS execution has not been verified by the Linux integration harness.

## Requirements

Media probing requires **ffprobe**, and extraction/encoding require **FFmpeg**. Package scripts include both alongside the executable; source builds find them on `PATH`. Existing optional gifsicle and Anime4K helper discovery is reused. Anime4K remains opt-in and requires the same helper as GUI exports. No additional hardware or GPU is required for ordinary headless operations.

Headless settings start from Weebshot defaults and are session-local. Use `export_set_settings` or per-job `settings` patches; headless operation never rewrites desktop preferences, recent files, or selections. An attached session reads the running GUI's settings and selected tracks, including edited and external subtitles. Settings patched through GUI attachment appear in the normal export controls and follow the GUI's normal persistence behavior.

## Client configurations

For a client using the `mcpServers` convention, such as Claude Desktop, merge this into its MCP configuration (use the absolute path to your executable):

```json
{
  "mcpServers": {
    "weebshot": {
      "command": "/absolute/path/to/weebshot",
      "args": ["--headless", "--mcp"]
    }
  }
}
```

For GUI attachment, replace `args` with `["--mcp"]`, then enable integration in the already-running window. The **Copy MCP client configuration** button copies this attachment variant with the current executable's absolute path. Paths containing spaces are ordinary JSON strings, without shell quoting. MCP clients launch the executable directly; never wrap stdout in shell messages.

VS Code uses a different wrapper in `.vscode/mcp.json`:

```json
{
  "servers": {
    "weebshot": {
      "type": "stdio",
      "command": "/absolute/path/to/weebshot",
      "args": ["--headless", "--mcp"]
    }
  }
}
```

## Tools and arguments

`tools/list` provides descriptions, JSON schemas, required fields, enums, and numeric bounds. Arguments reject unknown fields. Times are finite seconds from the start; `frame` is a zero-based position calculated using the reported frame rate. All paths are local filesystem paths. Relative output paths retain the existing rule: they resolve in the executing Weebshot process's working directory (the GUI process for attachment, the headless process otherwise), not the client's directory. See the explicit output-base option below; absolute paths are recommended.

| Tool | Arguments and result |
| --- | --- |
| `video_timeline_scenes` | Optional `start_seconds` (0), `end_seconds` (duration), `threshold` (0.35, 0.01–1), `max_scenes` (100, 1–1000), `min_scene_seconds` (0.25, 0.05–10), `include_thumbnails` (false), `frame_width` (240), `columns` (4). Returns approximate shot bounds and midpoint representatives; optional image uses the shared sheet renderer. |
| `report_issue` | Required `title`, `description`; optional `severity` (`debug`, `info`, `warning`, `error`; default warning), `component`, `tool_name`, `error_message`, `reproduction_steps`, `related_job_ids`. Writes an explicit structured event to existing Logs and acknowledges with `report_id`. |
| `video_contact_sheet` | Optional `start_seconds` (0), `end_seconds` (duration), `sampling` (`interval`, `uniform`, `scene`; default `interval`), `interval_seconds` (10), `columns` (4, 1–6), `frame_width` (480), `max_frames` (24, 1–64), `include_timestamps` (true). Returns a JPEG image directly with frame/PTS/tile metadata. No output path or job. |
| `video_extract_frames` | Required `timestamps_seconds` array (1–16 entries). Optional `max_width` (1280), `format` (`jpeg` or `png`, default `jpeg`), `quality` (JPEG, 85, 1–100). Returns indexed image blocks directly, with actual PTS and per-frame errors. No output path or job. |
| `media_open` | Required `path`; optional `replace_selection`. Validates the video before replacing active media. |
| `media_info` | No arguments. Duration, resolution, frame rate, codecs, tracks and container metadata. |
| `media_tracks` | No arguments. Tracks and selected audio/subtitle identifiers. |
| `media_select_track` | Required `kind` (`audio` or `subtitle`), and exactly one of `track_id` (integer or `null` to disable) or `external_path` (subtitle file). Optional `id_namespace`: `native` (default), `mpv`, `ffprobe`. GUI native IDs are mpv IDs; headless native IDs are ffprobe indices. |
| `media_close` | Optional `replace_selection`. Clears media and markers, leaving submitted jobs intact. |
| `timeline_get_state` | No arguments. Media, position, duration, frame rate, pause state and markers. |
| `timeline_seek` | Exactly one of `seconds` or `frame`. GUI seeks the visible player; headless updates the extraction cursor. |
| `timeline_set_in` | Optional `seconds` (defaults to cursor), `replace_selection`. |
| `timeline_set_out` | Optional `seconds` (defaults to cursor), `replace_selection`. |
| `timeline_set_range` | Required `in_seconds`, `out_seconds`; optional `replace_selection`, `seek_to_in`. Sets an ordered range atomically. |
| `playback_control` | Required `action`: `play`, `pause`, `step`; optional signed `frames` for step (default 1, bounded to ±1000). Headless supports pause/step; play requires a GUI. |
| `export_get_settings` | No arguments. Existing export/capture settings. |
| `export_set_settings` | Required `settings` object, patched into existing settings. |
| `screenshot_extract` | Optional absolute `base_directory`; required `output` ending in `.png`, `.jpg`, `.jpeg`, or `.webp`. Optional `seconds` or `frame` (otherwise cursor), `subtitles` boolean and `crop`. Returns `job_id`. |
| `export_add` | Optional absolute `base_directory`; required `output`; optional `in_seconds`, `out_seconds`, `settings`, `crop`. Uses markers, falling back to full media boundaries. Extension must match the selected clip format. Returns `job_id`. |
| `stitch_create` | Optional absolute `base_directory`; required `output` (`.png` or editable `.psd`). Optional `in_seconds`, `out_seconds`, `method`, `sampling_fps` (1–30), `search_seconds` (0.1–60), `layer_divider_percent` (10–90). `prefer_background` is accepted for compatibility and ignored. Uses both markers or automatic boundary discovery. Returns `job_id`. |
| `jobs_list` | No arguments. Automation jobs, status, progress, destination and results. |
| `jobs_get` | Required `job_id`; optional `preview`. Completed PNG/JPEG screenshot and PNG Stitch jobs with `preview:true` also return MCP image content: a PNG thumbnail bounded to 512×512. Full images remain at the requested destination. |
| `jobs_cancel` | Required `job_id`. Requests cancellation of queued/active work. Poll until terminal status. |
| `gui_get_state` | No arguments. GUI playback, markers, tracks, export controls, queue summary, pending preparation and errors. |
| `gui_open_media` | Same arguments as `media_open`, requires GUI. |
| `gui_seek` | Same arguments as `timeline_seek`, requires GUI. |
| `gui_set_range` | Same arguments as `timeline_set_range`, requires GUI. |
| `gui_set_export_settings` | Required `settings`, requires GUI. |
| `gui_prepare_export` | Required `in_seconds`, `out_seconds`; optional `media`, `settings`, `seek_to_in`, `replace_selection`. Configures a selection for user review without starting an export. |

When attached, generic media/timeline/playback/settings tools also operate on the GUI. Automation jobs appear in its Queue tab, with progress and cancellation. `jobs_*` identifies automation jobs in that GUI bridge's lifetime; `gui_get_state.jobs` is a summary of the GUI queue, whose IDs are separate and must not be passed to `jobs_*`.

Opening media in mpv is asynchronous. `gui_open_media` returns with a loading state; poll until `duration_seconds > 0`. `gui_prepare_export` can open media and defer its markers/settings/seek until loading completes. Poll `gui_get_state` until `preparing:false`, then inspect `automation_error` and the resulting selection. Preparations time out after ten seconds or are cancelled if the user changes media or sets markers while loading. A successful preparation never starts an export.

Existing markers require explicit `replace_selection:true` before replacement or closing/opening media. Replacing media with edited subtitles also requires it. Invalid ranges/settings are rejected before applying their changes. Transport commands preserve the current play/pause state except when explicitly asked to play, pause, step, or open media (opening normally pauses).

### Track identifiers and subtitles

`media_info.tracks` and `media_tracks.tracks` expose `mpv_id`, `ffprobe_index`, `type`, `language`, `codec_name`, `selected` and external-file information. Existing aliases (`id`, `index`, `codec_type`) remain for compatibility. Headless `mpv_id` is `null`; it never invents an mpv namespace. `media_info.streams` remains the original ffprobe metadata in headless mode. The response identifies `track_id_namespace`.

A GUI subtitle may have `mpv_id:1` and `ffprobe_index:2`. Select it explicitly with either:

```json
{"name":"media_select_track","arguments":{"kind":"subtitle","track_id":1,"id_namespace":"mpv"}}
{"name":"media_select_track","arguments":{"kind":"subtitle","track_id":2,"id_namespace":"ffprobe"}}
```

Omitting the namespace preserves existing scripts: native means mpv for GUI attachment and ffprobe for headless. Numeric IDs are never guessed across namespaces. Invalid selections enumerate valid IDs, languages and codecs; ambiguous external stream indices require an mpv ID. Headless mode explicitly rejects `id_namespace:"mpv"`.

For a separate subtitle file:

```json
{"name":"media_select_track","arguments":{"kind":"subtitle","external_path":"/media/episode.en.ass"}}
```

Headless external text subtitles support ASS, SSA, SRT and VTT. GUI attachment can use tracks already loaded in mpv, including external files; unsupported external bitmap burning returns an explicit error. Embedded text and bitmap support continues through the existing FFmpeg pipeline.

`screenshot_extract` with `subtitles:true` burns the **active selected subtitle source**, including a matching cue at the captured timestamp. `false` disables rendering; neither setting can remove subtitles baked into source pixels. A timestamp outside any cue produces no invented text. A missing active subtitle source returns an explicit warning alongside the job ID. The GUI's cached track ID updates before selection is acknowledged, so a subsequent submission sees the selected source immediately. Edited subtitles reuse the existing application source selection and are snapshotted as ASS for the worker. ASS styles/positions use the existing libass path. Missing libass, unreadable subtitle sources and unsupported rendering fail the job with an actionable error. Poll job status to distinguish submission from completed rendering.

Inspection frames, contact sheets and scene thumbnails intentionally omit subtitle tracks and leave the user's selected tracks intact.

### Output path resolution

Relative paths continue to use the executing process's working directory, not the GUI's capture-output directory setting. The MCP client/agent's working directory is not transmitted by the protocol. `gui_get_state.output_base_directory` and `timeline_get_state.output_base_directory` report the default base; successful submissions return absolute `output`, `outputs`, and the effective `base_directory`. Job metadata also retains the resolved destination/base.

To choose a base explicitly without changing any process directory:

```json
{
  "name":"screenshot_extract",
  "arguments":{
    "output":"weebshot-work/frame.png",
    "base_directory":"/home/user/project",
    "subtitles":false
  }
}
```

The base must be an absolute existing directory. This example requires `/home/user/project/weebshot-work` to already exist. No parent directories are created. Absolute output paths remain valid. Errors identify the resolved unavailable parent, for example `Output directory does not exist or cannot be accessed: /home/user/project/weebshot-work`. The base is a resolution rule, not a new filesystem sandbox; normal user permissions and no-overwrite checks still apply.

### Scheduling and responsiveness

Screenshot/clip/Stitch submissions capture GUI state and enter a bounded FIFO dispatcher. The GUI thread does not probe tools, decode images, reserve output files, run extraction or publish completed artifacts. Workers reuse the tools detected at GUI startup (or explicit tool redetection). Successful submissions acknowledge actual enqueueing with a job ID; extraction finishes independently.

GUI command handling and asynchronous completions run in eframe's `App::logic` callback, including while the window is hidden or minimized. Commands do not restore the window or require visible UI painting. Keeping this work in `App::ui` would suspend it when eframe stops drawing a hidden window, causing GUI-dependent requests to expire while cached job queries still succeed.

`jobs_get`, `jobs_list`, and cancellation use a shared status cache/atomic cancellation flags in the socket dispatcher. They do not need a GUI repaint or wait behind decoding. Image previews, media preflight probing, and explicit diagnostic-log acceptance use a fixed two-worker auxiliary pool. Status may lag worker events briefly; cancellation is asynchronous, so poll until a terminal state. Existing state names remain `queued`, `running`, `done`, `failed`, `cancelled`.

The explicit bounds are:

- 64 active local IPC connections, 16 FIFO GUI commands, 16 auxiliary requests and two auxiliary workers.
- 32 FIFO automation submissions and 32 active/queued processing jobs per automation engine. Completed history is separately bounded to 1000 jobs.
- One processing worker per automation engine, separate from manually submitted GUI work.
- Per stdio process, 32 ordinary in-flight calls plus eight reserved job-control calls; image previews have a separate two-operation limit and do not consume the cached-control lane. Visual inspection retains its separate two-operation bound.

Saturation returns a `busy`/queue-full error with capacity and retry guidance, rather than waiting for a GUI timeout. GUI-control deadlines are three seconds; submissions have ten seconds, and media preflight plus GUI opening has fifteen. Local transport waits at most twenty seconds. Queued commands expire before application, and disconnected/cancelled callers invalidate their pending commands. If cancellation races actual submission, any newly created unacknowledged jobs are cancelled; after an acknowledged submission, use `jobs_cancel` with its returned ID.

A controlled reproduction on the previous code delayed a second `gifsicle --version` probe by four seconds: one of four submissions timed out and the status query waited 4.10 seconds. The concrete cause was synchronous submission I/O on the GUI thread, amplified by a listener accepting one client at a time and a frontend mutex held across the entire GUI round trip. This is starvation, not a four-request execution limit. The original session's exact trace is unavailable; the regression suite reproduces this blocking path and checks 1/3/4/8 concurrent submissions, saturation, independent status reads and queued/running cancellation.

### Export settings

Settings patches use the existing serialized names and enum values:

- `clip_format`: `Webm`, `Mp4`, `Mkv`, `Gif`, `Webp`; `quality`: `Small`, `Balanced`, `High`. Quality applies the existing animation preset unless `anim` is supplied explicitly.
- `clip_stream_copy`, `include_audio`, `use_target_size`: booleans; `target_mb`: 0.01–100000 MiB. Stream copy supports MP4/MKV and cannot crop or upscale.
- `clip_subs`, `shot_subs`: `Off`, `Burned`, `Both`. A selected subtitle track is needed to burn subtitles. Headless starts with subtitles disabled; select one with `media_select_track`.
- `shot_format`: `Png`, `Jpg`, `Webp`. The explicit screenshot output extension determines its encoding.
- `max_height`, `anim_height`, `webp_height`: integer 2–16384 or `null`; `max_fps`: 1–240 or `null`; `anim_fps`: 1–120; `loop_count`: 1–100.
- `anim`: partial object with `colors` (2–256), `dither` (`None`, `Bayer`, `FloydSteinberg`, `Sierra`), `dedupe`, `rect_diff`, `gifsicle_optimize`, `gifsicle_lossy` (0–200).
- `encoder`: partial object with `x264_preset` (existing presets `ultrafast` through `veryslow`), `vp9_cpu_used` (0–5), `hw` (`null`, `Nvenc`, `Qsv`, `Amf`). Unsupported encoders fail as ordinary jobs.
- `shot_anime4k`, `clip_anime4k`: partial objects with `enabled`, `model` (`AcNet`, `ArNet`), `speed` (`Fast`, `Balanced`, `Slow`), `variant` (`Neutral`, `Denoise`, `Lines`, `LinesDenoise`), `device` (`Auto`, `Gpu`, `Cpu`).

`crop` is an explicit source-pixel rectangle: `{"x":0,"y":0,"w":640,"h":360}`. It must fit inside the source. GUI view zoom/cropping does not implicitly change automation exports; pass the rectangle explicitly.

Stitch methods are `automatic`, `translation`, `two_vertical_layers`, `shift_and_scale`, `layered_motion`. These are the current reconstruction methods, including their existing experimental limitations. Stitch does not implement contact sheets or arbitrary grid layouts.

### Example workflow

These are `tools/call` parameter objects, sent sequentially after MCP initialization:

```json
{"name":"media_open","arguments":{"path":"/media/episode.mkv"}}
{"name":"timeline_set_range","arguments":{"in_seconds":745.5,"out_seconds":758.2}}
{"name":"export_add","arguments":{"output":"/exports/scene.mp4","settings":{"clip_format":"Mp4","quality":"High","clip_subs":"Off"}}}
{"name":"jobs_get","arguments":{"job_id":1}}
```

For review in an attached window:

```json
{
  "name": "gui_prepare_export",
  "arguments": {
    "media": "/media/episode.mkv",
    "in_seconds": 745.5,
    "out_seconds": 758.2,
    "seek_to_in": true,
    "replace_selection": true,
    "settings": {"clip_format": "Mp4", "quality": "High"}
  }
}
```

A frame and its preview:

```json
{"name":"screenshot_extract","arguments":{"frame":17892,"subtitles":false,"output":"/exports/frame.png"}}
{"name":"jobs_get","arguments":{"job_id":2,"preview":true}}
```

Jobs transition through `queued`, `running`, then `done`, `failed`, or `cancelled`. Progress is a fraction between 0 and 1. Polling is safe while exports run, and completed output is published even if no polling occurs. Results contain the destination or an actionable error. Tool failures use MCP's `isError` and structured error data; protocol/argument-shape errors may be returned as JSON-RPC errors by the SDK. Never infer success solely from receiving a JSON-RPC response.

## Multi-frame visual inspection

These tools operate on the currently opened media through an independent software decoder. They do not seek the player, change play/pause state or tracks, edit markers/settings, create visible files, or add queue entries. In GUI attachment mode, the MCP process reads a versioned media snapshot from the GUI and performs decoding itself; it does not send a decoding operation to the UI thread. Headless mode uses the same backend without initializing egui.

For a broad episode overview, then a narrower scene and detailed expressions:

```json
{"name":"video_contact_sheet","arguments":{"sampling":"uniform","max_frames":24}}
{"name":"video_contact_sheet","arguments":{"start_seconds":900,"end_seconds":990,"interval_seconds":3,"max_frames":30}}
{"name":"video_contact_sheet","arguments":{"start_seconds":950,"end_seconds":960,"sampling":"interval","interval_seconds":0.25,"columns":4,"frame_width":480,"max_frames":40}}
{"name":"video_extract_frames","arguments":{"timestamps_seconds":[956,957.5,959],"max_width":1280,"format":"jpeg","quality":85}}
```

Use `gui_seek` or the existing capture/export tools separately after choosing a frame. Inspection itself leaves the user's position and selection intact.

An episode overview locates candidate regions; it does not establish a best expression or composition. For example, 48 frames over a 1420-second episode are almost 30 seconds apart. Tiny 200-pixel thumbnails can conceal facial details, and a brief appearance can fall entirely between samples. Rescan promising intervals at 480 pixels with 0.25–1 second spacing, then compare up to 16 closely spaced frames at 1280 pixels or larger. Use returned `actual_timestamp_seconds` for the final capture. Timestamps follow actual video presentation times, including variable frame rates; denser sampling improves temporal coverage rather than changing seek accuracy. Character identification and aesthetic judgments remain the calling agent's responsibility.

Scene detection analyzes the entire requested interval before selecting representatives. Reducing `max_frames`, columns or thumbnail width does not reduce that analysis. Prefer uniform sampling for a fast episode overview, then scene detection on a short candidate interval. Inspection has a configurable 120-second server deadline, independent of the client's request timeout. A client that times out earlier may cancel a healthy operation. Configure its per-server timeout above the server deadline; increasing only Weebshot's deadline cannot extend the client's wait.

For Oh My Pi (OMP), the installed harness defaults MCP requests to 30 seconds. Set `timeout` in milliseconds in its `~/.omp/agent/mcp.json` server entry, then reconnect the Weebshot MCP server (or restart the harness):

```json
{
  "mcpServers": {
    "weebshot": {
      "command": "/absolute/path/to/weebshot",
      "args": ["--mcp"],
      "timeout": 180000
    }
  }
}
```

OMP's `OMP_MCP_TIMEOUT_MS` environment variable overrides the per-server value. Prefer the per-server setting when only Weebshot needs the longer wait. Scene detection may still reach Weebshot's own limit; request a shorter interval or configure `WEEBSHOT_INSPECTION_TIMEOUT_SECONDS` in the server's environment with a correspondingly longer client timeout.

Sampling is deterministic:

- **Interval:** sample `start + n × interval_seconds`, excluding `end`. When there are more candidates than `max_frames`, keep the first `max_frames` and report `truncated:true` and `candidate_count`; density is never silently reduced.
- **Uniform:** use the centers of `max_frames` equal time bins. This avoids sampling exactly at the end of the file. For a 12-second interval and three frames, targets are 2, 6, and 10 seconds.
- **Scene:** FFmpeg scene-change scores (default threshold 0.3, configurable with `threshold`, on a small analysis frame) delimit scenes. Choose scene midpoints, rather than the cut itself. If there are too many scenes, choose distinct scene representatives nearest uniform time bins across the interval and report truncation. With no detected cuts, report `scene_fallback:true` and `effective_sampling:"uniform"`. Scene mode uses one analysis pass followed by the same seek-group extraction, with no model or extra dependency.

Extraction chooses the first decodable video frame **at or after** each target. It uses presentation timestamps directly, without resampling to nominal FPS. Frames that span several requested targets may therefore be repeated. Batch results retain request order, including duplicates. If no later decodable frame exists (for example in a trailing timestamp gap), that request gets a per-frame error rather than an invented image or PTS. Successful frames survive other decoding failures. Cancellation, media changes, timeouts and excessive payload sizes fail the operation explicitly.

`timestamp_seconds` is the requested target. `actual_timestamp_seconds` and `timecode` identify the extracted picture, and contact-sheet labels display that actual timecode. `presentation_timestamp` plus `time_base` preserve decoder timing precision. Values use the same zero-based media timeline as seeking: nonzero container timestamp origins are normalized. `image_index` addresses the image-only sequence; `content_index` addresses its image block in the full MCP `content` array. Each image is also preceded by a concise description. Contact-sheet `tile` rectangles locate pictures within the shared image.

A batch response's structured metadata resembles:

```json
{
  "frame_count": 2,
  "requested_count": 3,
  "partial": true,
  "frames": [
    {"index":0,"timestamp_seconds":1.1,"actual_timestamp_seconds":1.125,"timecode":"00:00:01.125","image_index":0,"content_index":2},
    {"index":1,"timestamp_seconds":2,"actual_timestamp_seconds":2,"timecode":"00:00:02.000","image_index":1,"content_index":4},
    {"index":2,"timestamp_seconds":2.99,"error":"No decodable frame at or after this timestamp"}
  ]
}
```

Images are real MCP JPEG/PNG content blocks. Base64 is confined to those blocks and never embedded in metadata/text. Clients must support standard MCP image content; there is no standard capability negotiation for a client's vision or image-token budget. Weebshot enforces its response limit and returns an actionable error recommending fewer frames/lower resolution, rather than substituting file paths.

Rotation metadata and sample aspect ratio are applied before encoding. Images use square display pixels and preserve the full frame, without cropping or subtitles. A contact sheet has a neutral dark background with labels underneath thumbnails; very tall frames fit within the cell and height limit. Batch extraction does not upscale beyond display size. JPEG quality controls only JPEG encoding; PNG ignores it.

The shared bounded FFmpeg pipe reader streams one resized image at a time. Targets at most five seconds apart share one decoder pass. Larger gaps start a new seek group, so sparse episode overviews skip intervening footage. Groups run sequentially, with only one decoder active per inspection; dense batches reuse that decoder rather than starting one per frame. Each group seeks to its first target and stops after its last target. Scene detection still scans the requested interval in one analysis pass. Output images/metadata are bounded in memory, and no full-video frame cache is retained. There is no cache in this initial implementation.

### Inspection limits

Defaults are 64 contact-sheet targets, 16 batch targets, two concurrent inspections per MCP process, a 1920-pixel requested-width ceiling, 2160-pixel frame-height ceiling, 8192-pixel sheet dimensions, 8 million decoded-output/canvas pixels, an 8 MiB serialized response budget (including base64), a 32-megapixel source-frame ceiling, and a 120-second decoding deadline. Configure the MCP process environment if needed:

| Variable | Default | Allowed range |
| --- | --- | --- |
| `WEEBSHOT_INSPECTION_MAX_WIDTH` | 1920 | 128–4096 |
| `WEEBSHOT_INSPECTION_MAX_CONTACT_FRAMES` | 64 | 1–64 |
| `WEEBSHOT_INSPECTION_MAX_BATCH_FRAMES` | 16 | 1–16 |
| `WEEBSHOT_INSPECTION_MAX_PIXELS` | 8000000 | 16384–32000000 |
| `WEEBSHOT_INSPECTION_MAX_RESPONSE_BYTES` | 8388608 | 1024–12582912 |
| `WEEBSHOT_INSPECTION_TIMEOUT_SECONDS` | 120 | 1–3600 |

Pixel limits cover Rust's resized-frame buffers and sheet canvas; FFmpeg retains the native decoder/reference surfaces its codec requires, using two decoding threads. Sources above the fixed source-frame ceiling are rejected. Scene detection retains at most 20000 boundaries; exceeding this asks for a shorter interval. The detector enforces a minimum shot interval (0.25 seconds by default) to suppress rapid frame-change/flash noise. Width arguments must also satisfy the tool schema (128–4096). Invalid environment settings produce an actionable tool error.

MCP cancellation notifications terminate the inspection decoder and reap it. EOF/SIGTERM also joins active inspection work. Changing or unloading media, changing the source file's identity/version, disconnecting the GUI, or disabling its integration cancels inspection; results from a stale media context are not returned.

## Approximate shot discovery

```json
{"name":"video_timeline_scenes","arguments":{"start_seconds":0,"end_seconds":1440,"threshold":0.35,"max_scenes":100,"include_thumbnails":false}}
```

The result includes `scenes` with original shot `index`, `start_seconds`, `end_seconds`, and `representative_seconds`, plus `total_scene_count`, returned `scene_count`, `truncated`, and the analysis method/settings. Representatives are midpoints. When shots exceed `max_scenes`, representatives are distributed by temporal bins across the interval rather than returning only the beginning. With no detected cuts, one shot spans the interval. This is approximate hard-cut discovery, not semantic scene understanding: pans, flashes, fades and animation can still cause misses or false positives. Threshold and `min_scene_seconds` are exposed and reported so clients can tune and reproduce the analysis.

`include_thumbnails:true` returns the same labelled JPEG grid and image/PTS mappings as contact sheets, with `scene_index` links and actual representative timestamps. Use `max_scenes:64` or fewer for thumbnail output; it must fit the configured contact-sheet frame, pixel and response bounds. Excessive thumbnail output returns an actionable error recommending fewer scenes or metadata-only output. Metadata-only discovery supports up to 1000 returned shots. Neither variant changes playback or adds queue entries.

## Explicit diagnostic reports

```json
{
  "name":"report_issue",
  "arguments":{
    "severity":"warning",
    "component":"mcp.gui_bridge",
    "title":"Concurrent submission failed",
    "description":"Observed a queue-full error during a burst of requests.",
    "tool_name":"screenshot_extract",
    "error_message":"Submission queue full",
    "reproduction_steps":["Submit a burst of captures","Poll existing jobs"],
    "related_job_ids":[42]
  }
}
```

Only explicitly supplied diagnostic content is recorded; the tool does not collect environment variables, secrets, file contents or private paths automatically. It never executes reproduction steps, writes an arbitrary destination, contacts a service, or submits anything to GitHub.

The existing Logs buffer stores native structured event fields under `event:"mcp.agent_issue_report"`, with native severity/source, a generated `report_id`, and `timestamp_unix_ms`. The Logs panel displays/filter/copy path preserves these fields. A single bounded asynchronous log sink emits the event as one JSON line on stderr in both debug and release builds; stdout stays reserved for MCP. An existing collector can parse/filter that JSON (for example Loki's `| json | event="mcp.agent_issue_report"`). No collector or Loki dependency is added. Logs remain the application's existing bounded in-memory diagnostics, not a durable bug database; external retention depends on the user's existing collector.

Safeguards: five reports per minute per receiving process; at most 200 title characters, 4096 description characters, 80 for component/tool name, 2048 for an error; 16 reproduction steps of 512 characters, 32 related job IDs, and 16 KiB total input. Control characters and direction/line-separator controls are sanitized. Unknown fields, oversize reports and rate excess are rejected. The log sink has a bounded 32-event queue. Acknowledgement occurs only after native logging accepts the event and returns its identifier; it makes no claim about triage, fixes or external delivery.

## Local IPC and security

There is no HTTP/TCP listener. The GUI bridge uses a user-owned directory with mode 0700 and socket mode 0600, by default `$TMPDIR/weebshot-mcp-<uid>/gui.sock` (`/tmp` when TMPDIR is unset). Other users cannot access that directory. The stdio frontend must use the same temporary-directory environment as the GUI, or specify its displayed path explicitly:

```sh
weebshot --mcp --mcp-socket /private/directory/gui.sock
```

The first enabled GUI owns that socket; additional instances report a settings-page error. A private `gui.sock.lock` file serializes bridge ownership and recovery; it remains on disk, while the OS releases its advisory lock when the process exits, including after a crash. Do not delete this lock file while Weebshot is running. On startup or re-enabling integration, an abandoned user-owned socket is reclaimed only after a bounded connection probe confirms connection refusal and its identity remains unchanged. Active, busy or uncertain sockets, regular files and symlinks are preserved. Normal shutdown removes only the socket created by that bridge. A GUI whose bridge failed to start can retry by toggling integration off and on.

After updating Weebshot, restart both the GUI and the MCP server process in your client. Restarting the GUI alone does not update the running stdio frontend or the client's cached tool discovery. If `video_contact_sheet` or another documented tool is missing, reconnect/restart the client's Weebshot MCP server so it launches the updated executable and refreshes its tools.

Clients run with your normal filesystem permissions. Trust connected clients as you would a local application: they can read media and create files in writable directories. No privilege escalation, network server, executable-path override, or arbitrary ffmpeg-argument interface is introduced. Media paths must be existing regular local files, metadata probing is bounded, timestamps and track IDs are checked, and settings use a restricted schema.

Output directories must already exist. Existing files and symlinks are rejected. Work is staged privately on the destination filesystem, then published with an atomic create-only hard link; even a file created by another process during encoding cannot be overwritten. Filesystems without hard-link support report a publication error. There is no force-overwrite option. Avoid manually modifying source/destination files during a job. Metadata probing permits only the file protocol; the existing FFmpeg pipeline handles actual media decoding.

## Current limits

- GUI IPC is Unix-only. Headless playback maintains a cursor; it does not play audio/video in real time.
- Video exports/extraction use the first video stream, as the existing pipeline does. Audio and subtitle tracks are selectable. Headless frame indices use nominal FPS; for variable-frame-rate sources, timestamps are preferable.
- `Both` subtitle mode with a selected subtitle queues two jobs: the requested output with subtitles and a sibling named `<stem>_raw.<extension>`. The response includes `job_id` for the first and `job_ids` for both. Explicit screenshot `subtitles` overrides this setting. Both destinations must be available before either job is queued.
- Export/capture jobs require explicit output files; their client images are bounded previews requested through `jobs_get`. The read-only `video_*` tools return inspection images directly without files or jobs. PSD and WebP previews are not returned (use PNG/JPEG capture for MCP image content). Stitch frame-selection editing remains in the GUI.
- A bridge has one serial automation worker, separate from the existing manually submitted GUI worker. It reuses `Runner`, screenshot filters, `ClipJob`, encoding presets, Anime4K, and Stitch/PSD implementations. It is not a distributed queue. At most 1000 automation jobs are retained per session/bridge lifetime.
- MCP exposes no prompts, resources, network transport, project serialization, or standalone non-MCP CLI export commands.

## Development verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --test mcp gui_attachment -- --ignored --nocapture
cargo test --test mcp visual_inspection_workflow -- --nocapture
cargo test --test mcp gui_bridge_reliability -- --ignored --nocapture
```

The Linux stdio integration test generates a deterministic video with audio, removes both DISPLAY and WAYLAND_DISPLAY, and verifies discovery, validation, metadata, tracks, cursor/markers, screenshot encodings and MCP image content, MP4/GIF/stream-copy exports, Stitch, failure/cancellation, sequential requests, EOF and SIGTERM cleanup. The GUI integration test requires Xvfb, Openbox, xdotool, xprop and software OpenGL, uses isolated preferences and socket directories, starts a real GUI, prepares a selection, checks markers/seeking/settings, exports a frame and checks disconnection. It also minimizes the window, verifies state/media/seek/range/settings and screenshot operations still work without restoring it, then restores it and checks the selection. Unit tests cover settings migration, schema validation, preset patching, unavailable media/GUI and IPC permissions/lifecycle.

Visual inspection tests additionally generate VFR, anamorphic, display-matrix rotation, scene-cut and static fixtures; check image/PTS mappings, response limits, partial errors and read-only state; and cancel slow real decoders through MCP cancellation, media/file changes, EOF and SIGTERM. The GUI test verifies inspection leaves its playhead, markers, tracks, settings and queue unchanged.

Reliability/media tests additionally cover the 1/3/4/8 concurrent submission matrix, cache polling with a stalled/saturated GUI lane, queue-full errors, disconnected commands, queued/running cancellation, known embedded/external ASS cues inside/outside cue ranges and styled pixels, explicit ID namespaces, resolved output errors, structured report acceptance/sanitization/rate limits, and approximate shot boundaries with optional thumbnails. Socket tests cover abandoned-socket recovery at real GUI startup, eight concurrent ownership attempts, live legacy listeners (including a full accept backlog), unsafe lock files, files/symlinks, lock reuse and replacement-path preservation on shutdown.
