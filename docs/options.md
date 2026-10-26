# Options & themes

Open with *File → Options…* ([Ctrl+,](key:Options)). Everything is saved automatically.

## General

**Save to** — a fixed folder, or next to the video.

**File names** — patterns for screenshots and clips, built from these tokens:

| Token | Becomes |
| --- | --- |
| `{name}` | the video file name without extension |
| `{time}` | current position, e.g. `12m03s417` |
| `{frame}` | current frame number |
| `{start}` / `{end}` | In / Out point |

Characters Windows doesn't allow in file names are replaced with `_`.

![Options → Playback](images/options.jpg)

### Sounds

A short sound plays when a screenshot is saved, when the export queue finishes, and when an export
fails. Turn them off or change the volume in *Options → General*; the *Test* buttons play each one.

### Updates

*Check GitHub for a new version once a day* (on by default) and a *Check now* button. See
[Installing & updating](install.md).

## Playback

- **Track selection** — preferred audio and subtitle languages (e.g. `jpn,ja` and `eng,en`), used to
  pick tracks when a file is opened.
- **Stepping** — how far the short, medium and long steps move
  ([←](key:FrameBack)/[→](key:FrameForward), with Shift, with Ctrl), each in **frames** or **seconds**.
  Defaults: 1 frame, 1 second, 5 seconds.
- **Hardware decoding** — turn it off if you see green frames or other glitches.

## Export

- **Tools** — shows which ffmpeg and gifsicle are used. Choose specific executables or auto-detect.
- **Encoders** — H.264 preset and VP9 speed.
- **Hardware encoding** — *Detect GPU encoders* tests NVIDIA NVENC, Intel Quick Sync and AMD AMF
  with a short encode and lists which work on your PC; pick one to encode MP4/MKV on the graphics
  card (see [Clips](clips.md)).

## Appearance

- **Theme** — Studio (dark, the default), Gruvbox Dark, Gruvbox Light, Nord, Catppuccin Mocha,
  Dracula, Light. Also in *View → Theme*.
- **Interface size** — scale the whole UI (0.75× – 2×).

![The Gruvbox Dark theme](images/theme-gruvbox.jpg)

## Shortcuts

Every action can be rebound. Click the shortcut, then press the new key combination
(Esc cancels). If that combination was used by another action, it moves — the status bar tells you
which action lost it. *Clear* unbinds, *Default* restores one, *Reset all* restores everything.

The [Keyboard shortcuts](shortcuts.md) page in this help always lists your current bindings.
