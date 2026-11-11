# Third-party software and assets

Weebshot itself is licensed under the GNU General Public License v3.0 or later (see `LICENSE`).
Release packages also contain or use the following.

## Bundled programs and libraries

| Component | License | Source |
| --- | --- | --- |
| **libmpv** (mpv) | GPL-2.0-or-later (these builds include GPL parts) | <https://mpv.io> · Windows builds: <https://github.com/shinchiro/mpv-winbuild-cmake> |
| **FFmpeg** | GPL-3.0 (Windows: gyan.dev shared build; macOS CLI: static build by Martin Riedl) | <https://ffmpeg.org> · <https://www.gyan.dev/ffmpeg/builds/> · <https://ffmpeg.martin-riedl.de> |
| **Anime4KCPP Core v3.2.0** | MIT | <https://github.com/TianZerL/Anime4KCPP/tree/v3.2.0> |
| **stb**, **ruapu**, **half** | MIT | <https://github.com/nothings/stb> · <https://github.com/nihui/ruapu> · <https://half.sourceforge.net> |
| **OpenCL headers and Windows ICD loader** | Apache-2.0 | <https://github.com/KhronosGroup/OpenCL-SDK> |
| **Linux OpenCL ICD loader (ocl-icd)** | BSD-2-Clause | <https://github.com/OCL-dev/ocl-icd> |
| **gifsicle** | GPL-2.0 | <https://www.lcdf.org/gifsicle/> · Windows builds: <https://eternallybored.org/misc/gifsicle/> |

The Linux AppImage and macOS app also contain the shared libraries these depend on, as shipped by
Ubuntu and Homebrew respectively, under their own licenses. Source code for all of the above is
available from the linked projects.

## Rust crates

Weebshot is built with egui/eframe, egui_commonmark, libmpv2, rfd, serde, image, ag-psd, crossbeam and
others — see `Cargo.toml` / `Cargo.lock` for the full list. They are MIT and/or Apache-2.0
licensed (libmpv2: LGPL-2.1).

## Assets

| Asset | License | Source |
| --- | --- | --- |
| Phosphor icons (`assets/fonts`) | MIT | <https://phosphoricons.com> |
| Camera shutter sound (`assets/sounds/shutter.wav`) | CC0 1.0 | "Camera Shutter Sound" by A Clock in the Kingdom, <https://archive.org/details/CameraShutterSound> |
| Fonts bundled by egui (Ubuntu, Hack, Noto Emoji, emoji-icon-font) | Ubuntu Font Licence, MIT, OFL | <https://github.com/emilk/egui> |
