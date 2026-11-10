# Auto Stitch

Reconstruct a larger illustration from a camera pan, entirely on your computer.
Both stitch actions use the same alignment and PNG export engine. They differ in
how they choose the video range:

| Action | Range used | Default shortcut |
| --- | --- | --- |
| Auto Stitch | Discovers the pan around the playhead, unless In/Out marks are set | [Shift+S](key:AutoStitch) |
| Stitch Selection | Uses the timeline's In/Out range; requires both marks | [Alt+Shift+S](key:StitchSelection) |

**When both In and Out are set, the two actions behave identically.** Clear the
marks with **Mark → Clear In/Out** ([Alt+X](key:ClearInOut)) to use automatic
discovery. A single mark must be completed or cleared before either action runs.

## Automatic workflow

Open a video, place the playhead inside a pan, clear any In/Out marks, and choose
**Export → Auto Stitch**. The action is also in the video context menu. Weebshot
searches backward and forward from the playhead, then opens a preview of the result.

## Stitching a specific range

Mark **In** ([I](key:SetIn)) and **Out** ([O](key:SetOut)) around the pan, then choose
**Export → Stitch Selection**, or right-click the timeline and choose
**Stitch Selection**. Stitching stays inside that range. Use this when automatic
discovery misses the intended section or you want only part of a pan.

## Preview and saving

The preview header shows the output dimensions, selected frame count, camera
direction, alignment confidence, and actual time range. Expand the amber warning
row to read why frames were omitted or the search limit was reached. **Details**
contains the full reconstruction notes and preview information. It starts fitted to
the window. Previews larger than 2,048 pixels are reduced for display; exports retain
all native pixels. Drag to pan, scroll or use the zoom buttons above the image, and choose **Fit**
to reset. The zoom multiplier is relative to the fitted view.
**Save PNG…** in the bottom-right corner opens a destination dialog using your screenshot output directory
(or the video's directory). Saving copies the exact native-resolution PNG behind
the preview; the video remains untouched.

**Retry** repeats the last stitch using the latest stitch settings. If the same
video is still open and both In/Out marks are set, it uses those current marks.
Otherwise it keeps the previous range or automatic search position. Seeking to a
new playhead position or clearing marks does not change an existing Retry request.
To start a fresh automatic search, close the preview, clear In/Out, move the
playhead, and activate **Auto Stitch** again.

## Choosing the source frames

Expand **Frames** above the image to see the aligned source-frame thumbnails and
video timestamps. Click a thumbnail or its checkbox to include or exclude it.
Excluded thumbnails are dimmed. Hover a thumbnail for a larger view of the pose.

Use **Apply frame selection** in the footer to rebuild the preview. This reuses
the solved alignment and runs composition in the background; it does not repeat
pan detection. The output dimensions, time range and frame count update after the
rebuild. Saving is available once the changes have been applied, so the exported
PNG always matches the displayed selection.

**Select all** restores all listed candidates, **Deselect all** lets you start
with an empty selection, and **Revert changes** returns to the last applied
selection. Include at least two overlapping frames. If removing intermediates
leaves a gap, include an intermediate frame and apply again. A failed or cancelled
rebuild keeps the previous preview available.

The strip contains the usable frames chosen by adaptive selection, within the
retained stable pan. Automatically rejected frames with no reliable placement are
not offered. To analyse another section, adjust In/Out and retry. **Retry** repeats
detection and resets the frame choices; **Apply frame selection** edits the current
stitch only.

Stitch jobs share the background export worker with clips. The stitch window
shows queued state and progress for decoding, discovery, alignment and composition.
Playback, seeking, and editing remain available. **Cancel** stops the job;
closing the window also cancels it. Temporary PNGs are removed when discarded or
when the application exits normally.

## Settings

In **Options → Export → Auto Stitch**, adjust:

| Setting | Default |
| --- | --- |
| Analysis rate | 8 frames/second, increased automatically for difficult motion |
| Search in each direction | 15 seconds |
| Allow short pauses | Enabled (up to 0.75 seconds) |

The initial version always uses high quality native-resolution composition and
PNG output. Subtitle tracks and preview crops are excluded. Subtitles already
encoded into the video cannot be removed.

## Best results and limits

Use static artwork with a horizontal or vertical pan, or a small diagonal
translation. The matcher tolerates limited foreground animation and compression
noise, but cannot reconstruct zooms, rotation, perspective changes, or substantial
parallax. Confidence measures translation agreement; it is not a guarantee that
all animated features are clean. Inspect faces, line art, and overlaps before saving.
Pixels in overlaps come from individual source frames, without repeated averaging.
Overlap boundaries favour matching areas and avoid connected moving regions where
possible, reducing cuts through changing poses.
Diagonal paths can leave transparent corners.

Scene cuts, poor detail, direction reversals, insufficient overlap and unreliable
native-resolution matches stop processing when no stable sequence remains. If a
pose change invalidates only part of the pan, Weebshot keeps the connected stable
sequence and warns about omitted frames. The displayed time range describes the
actual result, including for an In/Out selection; it never searches outside your
selection. Automatic discovery ends at cuts, incompatible motion or long holds;
short pauses are allowed. If a
search limit is reached, a warning explains that the pan may continue. Use a manually
marked range when discovery misses the desired section. An explicit range containing
a cut is rejected rather than silently shortened.

Limits: a selection of 60 seconds, 1,000 analysis samples, 96 selected source frames,
16 megapixels per source frame, and a 32 megapixel output with at most 16,384 pixels
on either edge. A decoder invocation times out after 120 seconds. Shorter ranges
reduce processing time and memory use. Analysis is downscaled; composition preserves
source pixel detail and FFmpeg's automatic orientation. Non-square source pixels
are displayed at their source aspect ratio and saved with PNG pixel aspect metadata;
some image viewers ignore this metadata. HDR colour fidelity is not supported in
this initial version.

## Performance and troubleshooting

Stitching runs locally on the CPU. Processing time depends on video resolution,
codec, range length, storage speed, and the number of selected frames. A stitch
can also wait behind a clip export in the shared worker; the preview then shows
**Queued with exports**. Stitching uses CPU processing independently of the playback hardware-decoding setting.
For source builds, use `cargo run --release` for normal performance; debug builds
are slower at image processing.

If the actions seem identical, check the In/Out marks first. If discovery fails,
mark a short range around one continuous pan and use **Stitch Selection**. If the
result covers less time than requested, read the omitted-frame warning: pose
changes or unreliable alignment can leave only part of the range usable. If no
stable sequence remains, processing ends with an error. See the **Logs**
tab for technical details when reporting a persistent failure.
