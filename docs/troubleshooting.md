# Troubleshooting

When something goes wrong, open the **Logs** tab in the side panel (or click the red warning counter
in the status bar). It shows
messages from Weebshot, mpv and ffmpeg, and every ffmpeg command that was run — you can copy a
command and run it in a terminal to see the full output.

## "ffmpeg not found"

The release downloads include ffmpeg next to weebshot. If you removed it or built Weebshot yourself,
put `ffmpeg` next to the Weebshot program, install it so it's on your `PATH`
(Windows: `winget install Gyan.FFmpeg`; Linux: your package manager; macOS: `brew install ffmpeg`),
or choose it in *Options → Export*.

## Hardware encoding fails

*Detect GPU encoders* only lists encoders that worked in a test, but drivers change. If GPU exports
fail, update your graphics driver or switch *Use for MP4/MKV* back to *Off*. Hover a "not available"
entry to see what ffmpeg reported.

## The update didn't work

Updating needs to write to the folder Weebshot is in; if that's not allowed (e.g. under
`Program Files`), download the new version from the release page instead. The Logs tab shows what
went wrong. On macOS, updates are always manual.

## A GIF takes forever or is huge

Check the clip length and *Size* in the Clip section — see [GIFs](gifs.md). The *Building palette* step
reads the whole range before anything is written, so its bar animates instead of filling.

## The export failed

The Queue shows the last lines of ffmpeg's error; the Logs tab has everything. Common causes:

- The output folder isn't writable.
- *Fit into file size* with a limit too small for the clip length — shorten the clip or raise the limit.
- Image-based subtitles from a separate file can't be burned in (only ones inside the video can).

## Subtitles look different in the clip than in the preview

Clips are rendered by ffmpeg's libass, the preview by mpv's libass, using the same fonts from the MKV.
Differences are rare; if you see one, please report it with the log.

## The subtitle lane says "timing unavailable"

The subtitle track couldn't be read (see Logs). Image-based subtitles (PGS) have no text, so they
aren't shown in the lane — marking with [X](key:MarkSubtitleLine) still works while a line is on screen.

## Scrubbing feels slow

Seeking to an exact frame means decoding from the previous keyframe, which takes longer on
high-bitrate or HEVC/AV1 releases. Weebshot never lets seeks pile up, but each one still takes as
long as it takes. Tips: scrub zoomed out (fast keyframe previews), then fine-tune with the arrow keys
or the shuttle. Hardware decoding (*Options → Playback*) usually helps.

## Green or corrupted frames

Turn off *Hardware decoding* in *Options → Playback*.

## Auto Stitch fails or gives a shorter result

Use one continuous camera pan with stable artwork. Cuts, large pose changes, low
detail, zooms, rotation, and parallax can prevent reliable alignment. Try marking
a shorter In/Out range and choosing **Stitch Selection**. If only part of the pan
can be aligned, the preview warns about omitted frames and shows the actual output
range. Inspect the result before saving.

If Auto Stitch and Stitch Selection seem to do the same thing, both In/Out marks
are probably set: these always take precedence. Clear them with
[Alt+X](key:ClearInOut) to let Auto Stitch discover the boundaries. See
[Auto Stitch](stitching.md) for the full workflow, settings, and performance notes.

## Resetting everything

Settings are stored in one file. Delete it while Weebshot is closed to start fresh:

| System | Location |
| --- | --- |
| Windows | `%APPDATA%\weebshot\data\app.ron` |
| Linux | `~/.local/share/weebshot/app.ron` |
| macOS | `~/Library/Application Support/weebshot/app.ron` |
