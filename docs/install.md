# Installing & updating

Download the latest version from the
[Releases page](https://github.com/Melanpan/weebshot/releases/latest). Everything Weebshot needs is
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

## Fedora RPM

Download **`weebshot-x86_64.rpm`** and install it with:

```sh
sudo dnf install ./weebshot-x86_64.rpm
```

This installs a menu entry and the `weebshot` command. The RPM includes the same
media tools, player libraries, and Anime4K helper as the AppImage, so RPM Fusion
is not required for those features. System graphics and windowing dependencies
are resolved by dnf. The bundled runtime is private to Weebshot.

To upgrade, download the new RPM and run the same command. To uninstall:

```sh
sudo dnf remove weebshot
```

RPM installations use dnf for upgrades; the in-app update check opens the release
page. There is no automatic RPM repository configured.

## macOS (Apple Silicon)

1. Download **`weebshot-macos-arm64.zip`** and unzip it.
2. Move **weebshot.app** to *Applications*.
3. The app isn't notarized by Apple, so the first time, **right-click it → Open → Open**. After that
   it opens normally.

There is no build for Intel Macs. The macOS build is newer than the others and less tested — please report anything odd.

## Updates

Weebshot checks GitHub for a new version once a day. When there is one, an **Update to …** button
appears in the status bar. It opens a window with the release notes and these choices:

- **Update and restart** (Windows, Linux AppImage) — downloads the new version, replaces the
  current one and starts it again. Running exports are stopped, so let them finish first.
- **Open release page** — download it yourself (this is the download option for macOS and RPM installations).
- **Skip this version** — don't mention this version again.

You can also check by hand with *Help → Check for updates…*, and turn the daily check off in
*Options → General → Updates*. The check only asks GitHub which release is the latest; nothing
about you or your files is sent.

## Building it yourself

See the [README](https://github.com/Melanpan/weebshot#building-from-source).
