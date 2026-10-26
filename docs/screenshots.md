# Screenshots

Press [S](key:Screenshot) or click **Screenshot** in the inspector. To grab a clean frame without
subtitles regardless of the setting below, press [Ctrl+S](key:ScreenshotClean) (also in the
preview's right-click menu).

The frame is saved by the video player itself, so it is:

- **exactly the frame you see**, at the video's full resolution (not the window size),
- rendered with **the same subtitle renderer** as the preview, including fonts embedded in the MKV.

## Options

- **Format** — PNG (lossless, default), JPG (quality 95), or WebP (quality 92).
- **Subtitles**
  - *No subs* — clean frame.
  - *Burn in* — with the subtitles shown in the preview.
  - *Both* — saves two files at once, `…_subbed` and `…_raw`.

  Subtitle options are disabled when no subtitle track is selected.

## File names

Default: `{name}_{time}`, e.g. `[Group] Show - 01_12m03s417.png`. Change it in *Options → General*.
If a file with that name already exists, ` (2)`, ` (3)`, … is added — nothing is overwritten.
