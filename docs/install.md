# Installing & updating

Download the latest version from the
[Releases page](https://github.com/Melanpan/weebshot/releases/latest). Everything weebshot needs is
included — the video player (libmpv), ffmpeg for exports, and gifsicle for smaller GIFs.

## Windows

1. Download **`weebshot-windows-x64.zip`**.
2. Unzip it anywhere you like (for example `Documents\weebshot`) — there's no installer.
3. Run **`weebshot.exe`**.

Windows SmartScreen may warn about an unrecognised app the first time, because the builds aren't
code-signed. Click *More info → Run anyway*.

## Linux

1. Download **`weebshot-x86_64.AppImage`**.
2. Make it executable: right-click → *Properties → Allow executing*, or `chmod +x weebshot-x86_64.AppImage`.
3. Run it.

The AppImage works on current desktop distributions (built on Ubuntu 24.04; tested on Ubuntu,
Fedora and Arch). If it doesn't start, your system may be missing FUSE — install `libfuse2`
(`fuse2` on Arch), or run it with `--appimage-extract-and-run`.

## macOS (Apple Silicon)

1. Download **`weebshot-macos-arm64.zip`** and unzip it.
2. Move **weebshot.app** to *Applications*.
3. The app isn't notarized by Apple, so the first time, **right-click it → Open → Open**. After that
   it opens normally.

The macOS build is newer than the others and less tested — please report anything odd.

## Updates

weebshot checks GitHub for a new version once a day. When there is one, an **Update to …** button
appears in the status bar. It opens a window with the release notes and these choices:

- **Update and restart** (Windows, Linux AppImage) — downloads the new version, replaces the
  current one and starts it again. Running exports are stopped, so let them finish first.
- **Open release page** — download it yourself (this is the only option on macOS).
- **Skip this version** — don't mention this version again.

You can also check by hand with *Help → Check for updates…*, and turn the daily check off in
*Options → General → Updates*. The check only asks GitHub which release is the latest; nothing
about you or your files is sent.

## Building it yourself

See the [README](https://github.com/Melanpan/weebshot#building-from-source).
