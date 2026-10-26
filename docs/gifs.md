# GIFs

GIF is an old format: 256 colors per frame and no real video compression. A few seconds of
1080p anime can easily be 100+ MB. weebshot does a lot to keep GIFs small, but the biggest
wins are yours to choose:

1. **Keep it short.** 2–6 seconds is the sweet spot. A warning appears past 15 s.
2. **Keep it small.** *Size* defaults to 360p; 480p is plenty for most uses.
3. **Lower the frame rate.** 12–15 fps looks fine for anime, which is mostly animated at 8–12
   drawings per second anyway.

If the place you post to accepts it, **WebM or animated WebP** will be 5–20× smaller at better quality.

## How a GIF is made

The export runs in up to three steps (all visible in the Queue and Logs):

1. **Building palette** — ffmpeg looks at the whole clip and picks the best 256 (or fewer) colors.
   This step can't report progress, so the bar animates.
2. **Encoding GIF** — every frame is mapped to that palette.
3. **Optimizing (gifsicle)** — only if gifsicle is installed.

## Optimization options

![GIF settings in the inspector](images/gif-options.jpg)

Open *GIF optimization* in the Clip section. The **Small / Balanced / High** preset buttons set all
of these at once.

| Option | What it does |
| --- | --- |
| Colors | Fewer colors = smaller file, more banding. 128 is often fine for anime's flat colors. |
| Dither | How missing colors are faked. *Bayer* compresses best; *Floyd–Steinberg* / *Sierra* look smoother but are bigger; *None* is smallest and flat. |
| Drop duplicate frames | Anime holds drawings for 2–3 frames; repeated frames are merged into one longer frame. Timing is preserved. |
| Only redraw changed areas | Each frame only stores the rectangle that changed — big savings on talking-head scenes. |
| gifsicle -O3 | Lossless re-optimization of the finished GIF. |
| lossy | gifsicle's lossy compression. 30–80 typically shrinks the file 30–50% with little visible noise. |

The gifsicle options are greyed out until gifsicle is found (next to `weebshot.exe` or on `PATH`, or
set it in *Options → Export*).

## Animated WebP

Uses *Size*, *FPS*, *Quality* and *Drop duplicate frames*. Usually far smaller than a GIF and supports
full color, but not every site accepts it. *Size* is separate from the GIF one and defaults to 720p.
