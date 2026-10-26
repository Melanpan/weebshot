# Clips

1. Mark the range: [I](key:SetIn) / [O](key:SetOut), the `I`/`O` flags on the timeline, or
   [X](key:MarkSubtitleLine) for the subtitle line on screen. Export refuses to run without a range —
   so you never accidentally encode a whole episode.
2. Choose the format and options in the inspector's **Clip** section.
3. Press [E](key:ExportClip). The export appears in the **Queue** tab, and a chime plays when the
   queue is done (see *Options → General → Sounds*).

## Formats

| Format | Video / audio | Good for |
| --- | --- | --- |
| WebM | VP9 + Opus | Imageboards, Discord, the web. Best quality per MB. |
| MP4 | H.264 + AAC | Plays everywhere (phones, Twitter/X, messengers). |
| MKV | H.264 + Opus | Archiving a cut. |
| GIF | — | Places that only take GIFs. Large; see [GIFs](gifs.md). |
| Animated WebP | — | A much smaller GIF alternative where supported. |

Video is always converted to 8-bit 4:2:0 so clips from 10-bit releases play on any device.
Cuts are frame-accurate (the clip is re-encoded, not stream-copied).

## Quality or file size

- **Quality** — *Small*, *Balanced*, *High*. Constant quality; the file is as big as it needs to be.
- **Fit into file size** — pick 4 / 8 / 10 / 25 / 50 MB or type a value. Weebshot computes the
  bitrate and encodes in two passes so the result lands just under the limit. If the limit is too
  tight for the clip length it tells you before starting, and hints at a lower resolution when that
  would look better (e.g. *consider 720p*).

## Match preview (zoom)

Zoom into the preview (the zoom buttons under it, or Ctrl+wheel over the picture) and tick
**Match preview (zoom)** to export just the area you see. The crop is taken when you press
Export; its size is shown next to the checkbox. Burned-in text subtitles are drawn at their normal
size at the bottom of the cropped picture (image subtitles are cropped along with the frame).

## Resolution and audio

- **Resolution** — *Original* or a maximum height (1080p … 240p). Never upscales.
- **Frame rate** — *Original* or a maximum (60 … 12 fps). Never adds frames above the source rate.
- **Include audio** — uses the audio track selected under **Tracks**, downmixed to stereo.

## Subtitles

- *No subs*, *Burn in*, or *Both* (two files: `…_subbed` and `…_raw`, queued together).
- Styled ASS subtitles keep their fonts — the fonts attached to the MKV are used.
- Image subtitles (PGS from Blu-rays) are supported when they're inside the video file.
- External subtitle files that mpv loaded automatically (e.g. `episode.ass` next to `episode.mkv`)
  work too.

## Encoder speed

*Options → Export* sets the H.264 preset and VP9 speed. Slower = smaller files at the same quality.

## Hardware (GPU) encoding

MP4 and MKV can be encoded on the graphics card — NVIDIA NVENC, Intel Quick Sync or AMD AMF —
which is several times faster than the CPU, at the cost of slightly bigger files for the same
quality. In *Options → Export → Hardware encoding*, click **Detect GPU encoders**: Weebshot encodes
a short test clip with each one and lists which actually work with your GPU and drivers. Then pick
one under *Use for MP4/MKV*.

With *Fit into file size*, GPU encoders don't do the two-pass analysis; they use a capped bitrate
aiming a bit under the limit instead. WebM, GIF and WebP always use the CPU.
