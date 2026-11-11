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
**Auto Stitch Selection** on the **Stitch** tab, or right-click the timeline and choose
**Stitch Selection**. Stitching stays inside that range. Use this when automatic
discovery misses the intended section or you want only part of a pan.

## Preview and saving

The preview header shows the output dimensions, selected frame count, camera
direction, alignment confidence, and actual time range. Expand the amber warning
row to read why frames were omitted or the search limit was reached. **Details**
contains the full reconstruction notes and preview information. It starts fitted to
the window. Previews larger than 2,048 pixels are reduced for display. PNG exports
use the reconstruction's full resolution. Drag to pan, scroll or use the zoom buttons above the image, and choose **Fit**
to reset. The zoom multiplier is relative to the fitted view.
**Save PNG…** in the bottom-right corner opens a destination dialog using your screenshot output directory
(or the video's directory). Saving copies the exact native-resolution PNG behind
the preview; the video remains untouched.

**Save PSD…** writes a Photoshop document with the aligned frames on separate
pixel layers and the overlap seams on editable layer masks. The two-vertical-layer
method exports each side of a frame on its own layer. Layer names show source
timestamps. Open the PSD, select a mask thumbnail, and paint white to reveal
more of that frame or black to hide part of it. The PSD also includes a merged
image that matches the PNG. Apply any frame selection edits before saving.
If the PSD exceeds the export memory budget, include fewer frames or shorten
the range.

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

Adjust the method and **Advanced** settings on the **Stitch** tab. The playhead discovery window is
available in **Options → Export → Auto Stitch**.

| Setting | Default |
| --- | --- |
| Stitch method | Automatic |
| Frame spacing | 8% of a frame in camera travel |
| Analysis rate | 8 frames/second, adaptive in Automatic and Single translation |
| Search in each direction | 15 seconds |
| Allow short pauses | Enabled (up to 0.75 seconds) |

**Automatic** tries single translation first and can fall back to two vertical
layers if ordinary alignment fails. **Single translation** uses one shift per
frame for artwork that moves together. **Two vertical layers** aligns the left
and right sides independently across the whole search window or marked range.
Use a tight In/Out range and, if the split is visible in the artwork, set the
divider as a percentage from the left edge. Leave the divider automatic to try
the midpoint and thirds. **Shift and scale** is for artwork that pans and zooms
together. Use In/Out around a single shot. This method scales frames into the
first frame's coordinate system, so the exported image includes resampled pixels.
**Layered motion (experimental)** is for vertical pans where overlapping artwork
moves at different speeds. It requires an In/Out range. Choose **Foreground** to
favor faster-moving artwork such as characters, or **Background** to favor the
slower motion. Adjust **Frame spacing** to change the time between selected
frames, then choose **Retry**. The preview can contain visible seams in
other layers and transparent edges. Use **Save PSD…** to adjust the joins by hand.
The preview's **Details** menu shows the method used.

Lower **Frame spacing** to include more source frames and increase overlap
between successive selections. Raise it to process fewer frames when there is
ample overlap. Choose **Retry** after changing these settings.

The initial version always uses high quality native-resolution composition and
PNG output. Subtitle tracks and preview crops are excluded. Subtitles already
encoded into the video cannot be removed.

## Best results and limits

Use static artwork with a horizontal or vertical pan, or a small diagonal
translation. The matcher tolerates limited foreground animation and compression
noise. If ordinary alignment fails, Weebshot can try aligning two side-by-side
regions that pan vertically at different speeds. The preview warns when this is
used and shows the height covered by both regions. Inspect the join before saving.
Use In/Out marks around the pan if the automatic search window includes other shots.

Shift and scale supports a uniform zoom on one plane. Rotation, perspective
changes, and automatic separation of overlapping silhouettes remain unsupported.
Confidence measures agreement with the selected motion model. Inspect faces, line art, and overlaps before saving.
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
reduce processing time and memory use. Analysis is downscaled. Composition uses source pixels for translation-only
methods and resamples frames for Shift and scale. FFmpeg applies source orientation. Non-square source pixels
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
