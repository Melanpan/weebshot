# Screenshots

Press [S](key:Screenshot) or click **Take Screenshot** in **Capture → Screenshot**. To grab a clean frame without
subtitles regardless of the setting below, press [Ctrl+S](key:ScreenshotClean) (also in the
preview's right-click menu).

The frame is saved by the video player itself, so it is:

- **exactly the frame you see**, at the video's full resolution (not the window size),
- rendered with **the same subtitle renderer** as the preview, including fonts embedded in the MKV.

A gentle ding confirms a saved screenshot. Adjust its volume or disable sounds in
*Options → General → Sounds*.

## Copy to clipboard

Click **Copy** beside Take Screenshot or press [Ctrl+Shift+C](key:CopyScreenshot) to copy the
current frame as an image. This follows the selected subtitles and preview crop.
**Both** copies the burned-in version. The copied image can be pasted into an application
that accepts images.

## Options

- **Format** — PNG (lossless, default), JPEG (quality 95), or WebP (quality 92).
- **Subtitles**
  - *No subs* — clean frame.
  - *Burn in* — with the selected subtitle track.
  - *Both* — saves two files at once, `…_subbed` and `…_raw`.

  Subtitle options are disabled when no subtitle track is selected.

## Match preview (zoom)

When you zoom into the preview (the zoom buttons under it, or Ctrl+wheel over the picture),
**Match preview (zoom)** saves exactly what you see instead of the whole frame — handy for a
close-up of a face or a sign. The size of the crop is shown next to the checkbox. Burned-in
subtitles are drawn at their normal size at the bottom of the crop, like in the preview.

These screenshots are rendered by ffmpeg instead of the player, so they take a moment longer.

## Anime4K

Enable **Anime4K (2×)** in the Screenshot section to double both dimensions after cropping.
The setting applies to saved PNG, JPG, and WebP screenshots and to **Copy**. With
**Both** selected, screenshots save the subbed and raw frames, while Copy copies the
subbed frame. The queue shows progress and lets you cancel or retry.

Screenshot and clip settings are saved separately and start off. See the
[model and device controls](clips.md#anime4k) for the available choices.

## File names

Default: `{name}_{time}`, e.g. `[Group] Show - 01_12m03s417.png`. Change it in *Options → General*.
If a file with that name already exists, ` (2)`, ` (3)`, … is added — nothing is overwritten.

For artwork revealed by a camera pan, use [Auto Stitch](stitching.md) to combine
overlapping frames into a larger PNG.
