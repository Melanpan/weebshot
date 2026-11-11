//! Turns a [`ClipJob`] into ffmpeg (and gifsicle) command lines.

use std::path::Path;

use anyhow::{Result, ensure};

use super::{
    ClipFormat, ClipJob, Crop, HwEncoder, Quality, Step, SubSource, TempFiles, Tool, size,
};

/// Escape a path for use as a filter option value inside a filtergraph.
///
/// ffmpeg unescapes twice: once when splitting the filtergraph (`[ ] , ;` and
/// quotes are special) and once when parsing `key=value:key=value` options
/// (`:` and quotes are special). Backslashes are turned into forward slashes
/// first, which Windows accepts in paths.
pub fn escape_filter_path(path: &Path) -> String {
    fn escape(s: &str, special: &[char]) -> String {
        let mut out = String::with_capacity(s.len() * 2);
        for c in s.chars() {
            if c == '\\' || special.contains(&c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
    let s = path.to_string_lossy().replace('\\', "/");
    let option_level = escape(&s, &['\'', ':']);
    escape(&option_level, &['\'', '[', ']', ',', ';'])
}

/// Input pad(s) and filters for cropping and burning in subtitles, starting from the input
/// stream at `start` seconds into the source.
///
/// Text subtitles are burned in *after* cropping so they keep their normal size at the bottom
/// of the cropped picture, like the zoomed preview shows them. Image subtitles (PGS) are
/// positioned on the full frame, so those are overlaid first and cropped with it.
fn subtitle_chain(
    start: f64,
    subs: Option<&SubSource>,
    crop: Option<Crop>,
    preserve_offset: bool,
) -> (String, Vec<String>) {
    let mut filters: Vec<String> = Vec::new();
    let mut input = "[0:v:0]".to_string();
    match subs {
        Some(SubSource::Text { path, stream }) => {
            filters.extend(crop.map(Crop::filter));
            // Input-side -ss rebases timestamps to 0, but the subtitle file
            // isn't seeked, so shift into source time and back.
            filters.push(format!("setpts=PTS+{}/TB", secs(start)));
            filters.push(format!(
                "subtitles=filename={}:si={stream}",
                escape_filter_path(path)
            ));
            filters.push(if preserve_offset {
                format!("setpts=PTS-{}/TB", secs(start))
            } else {
                "setpts=PTS-STARTPTS".into()
            });
        }
        Some(SubSource::Bitmap { ff_index }) => {
            input = format!("[0:v:0][0:{ff_index}]");
            filters.push("overlay=format=auto".into());
            filters.extend(crop.map(Crop::filter));
        }
        Some(SubSource::Edited(_)) => {
            unreachable!("materialize edited subtitles before building filters")
        }
        None => filters.extend(crop.map(Crop::filter)),
    }
    (input, filters)
}

/// A single frame saved by ffmpeg — used for screenshots of a cropped (zoomed) view, which
/// mpv's own screenshots can't do.
#[derive(Clone, Debug)]
pub struct StillJob {
    pub input: std::path::PathBuf,
    /// Exact timestamp of the frame on screen.
    pub time: f64,
    pub fps: f64,
    pub crop: Option<Crop>,
    pub subs: Option<SubSource>,
    pub output: std::path::PathBuf,
}

impl StillJob {
    pub fn args(&self) -> Vec<String> {
        // Seek a quarter frame early: ffmpeg keeps the first frame at or after the seek
        // point, which is then exactly the frame that was on screen.
        let start = (self.time - 0.25 / self.fps.max(1.0)).max(0.0);
        let (input, chain) = subtitle_chain(start, self.subs.as_ref(), self.crop, false);
        let graph = if chain.is_empty() {
            format!("{input}null[v]")
        } else {
            format!("{input}{}[v]", chain.join(","))
        };
        let mut args: Vec<String> = strings([
            "-hide_banner",
            "-nostdin",
            "-y",
            "-loglevel",
            "warning",
            "-ss",
        ])
        .collect();
        args.push(secs(start));
        args.push("-i".into());
        args.push(self.input.to_string_lossy().into_owned());
        args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into()]);
        args.extend(strings(["-frames:v", "1", "-update", "1"]));
        let ext = self
            .output
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_ascii_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => args.extend(strings(["-q:v", "2"])),
            "webp" => args.extend(strings(["-c:v", "libwebp", "-quality", "92"])),
            // 10-bit sources would otherwise become 16-bit PNGs; mpv's own screenshots
            // are 8-bit (`screenshot-high-bit-depth=no`), so match that.
            "png" => args.extend(strings(["-pix_fmt", "rgb24"])),
            _ => {}
        }
        args.push(self.output.to_string_lossy().into_owned());
        args
    }
}

fn secs(t: f64) -> String {
    format!("{t:.3}")
}

fn null_output() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

fn strings<const N: usize>(a: [&str; N]) -> impl Iterator<Item = String> {
    a.into_iter().map(String::from)
}

impl ClipJob {
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }

    fn audio_kbps(&self) -> u32 {
        match self.format {
            ClipFormat::Webm | ClipFormat::Mkv => {
                if self.target_mb.is_some() {
                    96
                } else {
                    128
                }
            }
            ClipFormat::Mp4 => {
                if self.target_mb.is_some() {
                    112
                } else {
                    160
                }
            }
            ClipFormat::Gif | ClipFormat::Webp => 0,
        }
    }

    fn has_audio(&self) -> bool {
        self.format.has_audio() && self.audio_ff_index.is_some()
    }

    /// Input pad(s) and the filter chain shared by every output format:
    /// subtitles, scaling, frame rate / pixel format.
    fn video_chain(&self) -> (String, Vec<String>) {
        let (input, mut filters) = if self.anime4k.enabled {
            ("[0:v:0]".into(), Vec::new())
        } else {
            subtitle_chain(self.start, self.subs.as_ref(), self.crop, false)
        };

        let height = if self.anime4k.enabled {
            self.crop
                .map(|crop| crop.height)
                .or(self.src_height)
                .map(|h| h.saturating_mul(2))
        } else {
            self.src_height
        };
        if let Some(h) = self.max_height
            && height.is_none_or(|src| h < src)
        {
            filters.push(format!("scale=-2:{h}:flags=lanczos"));
        }

        if self.format.is_animation() {
            filters.push(format!("fps={}", self.fps));
            if self.anim.dedupe {
                filters.push("mpdecimate".into());
            }
            if self.format == ClipFormat::Gif {
                // BT.709 video makes palettegen warn about non-sRGB input; the RGB
                // conversion is unaffected (verified byte-identical), so just tag it.
                filters.push("setparams=color_trc=iec61966-2-1".into());
            }
            if self.format == ClipFormat::Webp {
                // Hand libwebp RGB: it converts with the matrix WebP decoders assume, and
                // scores better at the same size than ffmpeg's yuv420p.
                filters.push("format=bgra".into());
            }
        } else {
            if let Some(fps) = self.max_fps
                && self.src_fps.is_none_or(|src| f64::from(fps) < src - 0.01)
            {
                filters.push(format!("fps={fps}"));
            }
            // Anime releases are often 10-bit; force 8-bit 4:2:0 for playback compatibility.
            // (Quick Sync wants the same thing in its native NV12 layout.)
            filters.push(if self.hw() == Some(HwEncoder::Qsv) {
                "format=nv12".into()
            } else {
                "format=yuv420p".into()
            });
        }
        if self.loop_count > 1 && self.format.is_animation() {
            // The `loop` filter keeps up to `size` decoded frames in memory. That's fine
            // here: animations are scaled down and run at a low fixed rate, and `size`
            // is bounded by the clip's actual frame count (the filter's hard limit is
            // 32767). Video formats loop at the container level instead, see
            // `loop_step`, since a full-resolution clip would not fit in RAM.
            let frames = (self.duration() * f64::from(self.fps)).ceil() as u32 + 2;
            filters.push(format!(
                "loop=loop={}:size={}:start=0",
                self.loop_count - 1,
                frames.clamp(1, 32767)
            ));
        }
        (input, filters)
    }

    /// Repeat an already encoded clip by concatenating it with itself: `-stream_loop`
    /// re-reads the file and `-c copy` keeps the packets, so no frame is held in memory
    /// and nothing is re-encoded.
    fn loop_step(&self, single: &Path) -> Step {
        let mut args = Self::progress_args();
        args.extend([
            "-stream_loop".into(),
            (self.loop_count - 1).to_string(),
            "-i".into(),
            single.to_string_lossy().into_owned(),
        ]);
        args.extend(strings([
            "-c",
            "copy",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
        ]));
        if self.format == ClipFormat::Mp4 {
            args.extend(strings(["-movflags", "+faststart"]));
        }
        args.push(self.output.to_string_lossy().into_owned());
        Step {
            tool: Tool::Ffmpeg,
            args,
            label: "Repeating clip",
            weight: 0.1,
            // `out_time` runs to loop_count × duration; not comparable to the clip length.
            reports_progress: false,
        }
    }

    fn source_args(&self) -> Vec<String> {
        let mut args = Self::progress_args();
        args.extend(self.trimmed_input_args());
        args
    }

    fn progress_args() -> Vec<String> {
        strings([
            "-hide_banner",
            "-nostdin",
            "-y",
            "-loglevel",
            "warning",
            "-progress",
            "pipe:1",
            "-nostats",
        ])
        .collect()
    }

    fn trimmed_input_args(&self) -> Vec<String> {
        vec![
            "-ss".into(),
            secs(self.start),
            "-t".into(),
            secs(self.duration()),
            "-i".into(),
            self.input.to_string_lossy().into_owned(),
        ]
    }

    /// `with_audio = false` for steps that produce no audio (two-pass analysis): with
    /// Anime4K the audio comes from a second input, which those steps needn't open.
    fn input_args(&self, tmp: &TempFiles, with_audio: bool) -> Vec<String> {
        if !self.anime4k.enabled {
            return self.source_args();
        }
        let mut args = Self::progress_args();
        // The runner supplies the prepared video's first PTS. FFmpeg subtracts
        // the Matroska start time on input, while audio is relative to the trim.
        args.extend(strings(["-itsoffset", "0", "-i"]));
        args.push(
            tmp.passlog
                .with_extension("upscaled.mkv")
                .to_string_lossy()
                .into_owned(),
        );
        if with_audio && self.has_audio() {
            args.extend(self.trimmed_input_args());
        }
        args
    }

    fn upscale_steps(&self, tmp: &TempFiles) -> Vec<Step> {
        if !self.anime4k.enabled {
            return Vec::new();
        }
        let prepared = tmp.passlog.with_extension("prepared.mkv");
        let upscaled = tmp.passlog.with_extension("upscaled.mkv");
        let (input, mut filters) = subtitle_chain(self.start, self.subs.as_ref(), self.crop, true);
        filters.push("format=yuv444p16le".into());
        let mut args = self.source_args();
        args.extend([
            "-filter_complex".into(),
            format!("{input}{}[v]", filters.join(",")),
        ]);
        args.extend(strings([
            "-map",
            "[v]",
            "-an",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-fps_mode",
            "passthrough",
            "-enc_time_base",
            "1:1000",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
        ]));
        args.push(prepared.to_string_lossy().into_owned());
        vec![
            Step {
                tool: Tool::Ffmpeg,
                args,
                label: "Preparing Anime4K input",
                weight: 1.0,
                reports_progress: true,
            },
            self.anime4k.step("video", &prepared, &upscaled),
        ]
    }

    /// The GPU encoder in use, if any (only for H.264 formats).
    fn hw(&self) -> Option<HwEncoder> {
        self.encoder
            .hw
            .filter(|_| matches!(self.format, ClipFormat::Mp4 | ClipFormat::Mkv))
    }

    /// GPU H.264: constant quality, or a capped bitrate in target-size mode (these
    /// encoders don't do ffmpeg's two-pass, so a little headroom is left instead).
    fn hw_codec_args(&self, hw: HwEncoder, kbps: Option<u32>) -> Vec<String> {
        let q = match self.quality {
            Quality::Small => 30,
            Quality::Balanced => 25,
            Quality::High => 21,
        }
        .to_string();
        let mut a: Vec<String> = vec!["-c:v".into(), hw.codec().into()];
        match hw {
            HwEncoder::Nvenc => a.extend(strings(["-preset", "p6", "-tune", "hq", "-rc", "vbr"])),
            HwEncoder::Qsv => a.extend(strings(["-preset", "slower"])),
            HwEncoder::Amf => a.extend(strings(["-quality", "quality"])),
        }
        match kbps {
            None => match hw {
                HwEncoder::Nvenc => a.extend(["-cq".into(), q, "-b:v".into(), "0".into()]),
                HwEncoder::Qsv => a.extend(["-global_quality".into(), q]),
                HwEncoder::Amf => a.extend([
                    "-rc".into(),
                    "cqp".into(),
                    "-qp_i".into(),
                    q.clone(),
                    "-qp_p".into(),
                    q,
                ]),
            },
            Some(k) => {
                if hw == HwEncoder::Amf {
                    a.extend(strings(["-rc", "vbr_peak"]));
                }
                a.extend([
                    "-b:v".into(),
                    format!("{k}k"),
                    "-maxrate".into(),
                    format!("{k}k"),
                    "-bufsize".into(),
                    format!("{}k", k * 2),
                ]);
            }
        }
        a
    }

    fn video_codec_args(&self) -> Vec<String> {
        let q = self.quality;
        let mut args: Vec<String> = match self.format {
            ClipFormat::Webm => {
                let mut a: Vec<String> = strings([
                    "-c:v",
                    "libvpx-vp9",
                    "-row-mt",
                    "1",
                    "-deadline",
                    "good",
                    "-cpu-used",
                ])
                .collect();
                a.push(self.encoder.vp9_cpu_used.min(5).to_string());
                a
            }
            ClipFormat::Mp4 | ClipFormat::Mkv => {
                let mut a: Vec<String> = strings(["-c:v", "libx264", "-preset"]).collect();
                a.push(self.encoder.x264_preset.clone());
                a
            }
            ClipFormat::Gif => strings(["-c:v", "gif", "-loop", "0"]).collect(),
            ClipFormat::Webp => strings([
                "-c:v",
                "libwebp_anim",
                "-loop",
                "0",
                "-compression_level",
                "6",
            ])
            .collect(),
        };

        if self.target_mb.is_none() {
            let crf = match (self.format, q) {
                (ClipFormat::Webm, Quality::Small) => Some(38),
                (ClipFormat::Webm, Quality::Balanced) => Some(32),
                (ClipFormat::Webm, Quality::High) => Some(24),
                (ClipFormat::Mp4 | ClipFormat::Mkv, Quality::Small) => Some(26),
                (ClipFormat::Mp4 | ClipFormat::Mkv, Quality::Balanced) => Some(21),
                (ClipFormat::Mp4 | ClipFormat::Mkv, Quality::High) => Some(17),
                _ => None,
            };
            if let Some(crf) = crf {
                args.extend(["-crf".into(), crf.to_string()]);
                if self.format == ClipFormat::Webm {
                    args.extend(strings(["-b:v", "0"]));
                }
            }
            if self.format == ClipFormat::Webp {
                let qv = match q {
                    Quality::Small => 70,
                    Quality::Balanced => 85,
                    Quality::High => 92,
                };
                args.extend(["-q:v".into(), qv.to_string()]);
            }
        }
        if self.format.is_animation() && self.anim.dedupe {
            // Keep the variable frame timing mpdecimate produces.
            args.extend(strings(["-fps_mode", "vfr"]));
        }
        args
    }

    fn audio_args(&self) -> Vec<String> {
        let Some(idx) = self.audio_ff_index.filter(|_| self.format.has_audio()) else {
            return vec!["-an".into()];
        };
        let codec = match self.format {
            ClipFormat::Mp4 => "aac",
            _ => "libopus",
        };
        let args = vec![
            "-map".into(),
            format!("{}:{idx}", usize::from(self.anime4k.enabled)),
            "-c:a".into(),
            codec.into(),
            "-b:a".into(),
            format!("{}k", self.audio_kbps()),
            "-ac".into(),
            "2".into(),
        ];
        args
    }

    fn upscaled_video_timing(&self) -> impl Iterator<Item = String> {
        (self.anime4k.enabled && !self.format.is_animation())
            .then_some(["-fps_mode", "vfr", "-enc_time_base", "1:1000"])
            .into_iter()
            .flatten()
            .map(String::from)
    }

    fn output_args(&self) -> Vec<String> {
        let mut args: Vec<String> =
            strings(["-map_metadata", "-1", "-map_chapters", "-1", "-sn", "-dn"]).collect();
        args.extend(self.upscaled_video_timing());
        if self.format == ClipFormat::Mp4 {
            args.extend(strings(["-movflags", "+faststart"]));
        }
        args.push(self.output.to_string_lossy().into_owned());
        args
    }

    /// Video bitrate for target-size mode, if enabled.
    pub fn target_video_kbps(&self) -> Result<Option<u32>> {
        if self.stream_copy {
            return Ok(None);
        }
        match self.target_mb {
            Some(mb) if self.format.supports_target_size() => {
                let audio = if self.has_audio() {
                    self.audio_kbps()
                } else {
                    0
                };
                Ok(Some(size::video_kbps(mb, self.duration(), audio)?))
            }
            _ => Ok(None),
        }
    }

    /// Every process the job runs, in order.
    pub fn build_steps(&self, tmp: &TempFiles) -> Result<Vec<Step>> {
        if self.stream_copy {
            ensure!(
                self.format.supports_stream_copy(),
                "Copy is supported for MP4 and MKV only"
            );
            let mut args: Vec<String> = strings([
                "-hide_banner",
                "-nostdin",
                "-y",
                "-loglevel",
                "warning",
                "-progress",
                "pipe:1",
                "-nostats",
                "-ss",
            ])
            .collect();
            args.extend([
                secs(self.start),
                "-t".into(),
                secs(self.duration()),
                "-i".into(),
                self.input.to_string_lossy().into_owned(),
                "-map".into(),
                "0:v:0".into(),
            ]);
            if let Some(index) = self.audio_ff_index {
                args.extend(["-map".into(), format!("0:{index}")]);
            } else {
                args.push("-an".into());
            }
            args.extend(strings([
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
                "-map_metadata",
                "-1",
                "-map_chapters",
                "-1",
                "-sn",
                "-dn",
            ]));
            if self.format == ClipFormat::Mp4 {
                args.extend(strings(["-movflags", "+faststart"]));
            }
            args.push(self.output.to_string_lossy().into_owned());
            return Ok(vec![Step {
                tool: Tool::Ffmpeg,
                args,
                label: "Copying streams",
                weight: 1.0,
                reports_progress: true,
            }]);
        }
        if self.format == ClipFormat::Gif {
            let mut steps = self.upscale_steps(tmp);
            steps.extend(self.gif_steps(tmp));
            return Ok(steps);
        }
        if self.loop_count > 1 && !self.format.is_animation() {
            // Encode once into the staging dir, then repeat the file by stream copy.
            let single = tmp
                .passlog
                .with_extension(format!("single.{}", self.format.ext()));
            let once = Self {
                loop_count: 1,
                output: single.clone(),
                ..self.clone()
            };
            let mut steps = once.build_steps(tmp)?;
            steps.push(self.loop_step(&single));
            return Ok(steps);
        }

        let mut steps = self.upscale_steps(tmp);
        let (input, chain) = self.video_chain();
        let graph = format!("{input}{}[v]", chain.join(","));
        let with_graph = |mut args: Vec<String>| {
            args.extend([
                "-filter_complex".into(),
                graph.clone(),
                "-map".into(),
                "[v]".into(),
            ]);
            args
        };

        if let Some(hw) = self.hw() {
            // One pass on the GPU; aim 8% low since there's no analysis pass.
            let kbps = self.target_video_kbps()?.map(|k| k * 92 / 100);
            let mut args = with_graph(self.input_args(tmp, true));
            args.extend(self.hw_codec_args(hw, kbps));
            args.extend(self.audio_args());
            args.extend(self.output_args());
            steps.push(Step {
                tool: Tool::Ffmpeg,
                args,
                label: "Encoding (GPU)",
                weight: 1.0,
                reports_progress: true,
            });
            return Ok(steps);
        }

        let Some(kbps) = self.target_video_kbps()? else {
            let mut args = with_graph(self.input_args(tmp, true));
            args.extend(self.video_codec_args());
            args.extend(self.audio_args());
            args.extend(self.output_args());
            steps.push(Step {
                tool: Tool::Ffmpeg,
                args,
                label: "Encoding",
                weight: 1.0,
                reports_progress: true,
            });
            return Ok(steps);
        };

        let passlog = tmp.passlog.to_string_lossy().into_owned();
        let bitrate = format!("{kbps}k");
        for pass in ["1", "2"] {
            // Pass 1 writes no audio, so don't open the audio input for it.
            let mut args = with_graph(self.input_args(tmp, pass == "2"));
            args.extend(self.video_codec_args());
            args.extend(strings([
                "-b:v",
                &bitrate,
                "-pass",
                pass,
                "-passlogfile",
                &passlog,
            ]));
            if pass == "1" {
                args.extend(self.upscaled_video_timing());
                args.extend(strings(["-an", "-f", "null", null_output()]));
            } else {
                args.extend(self.audio_args());
                args.extend(self.output_args());
            }
            steps.push(Step {
                tool: Tool::Ffmpeg,
                args,
                label: if pass == "1" {
                    "Analyzing (pass 1/2)"
                } else {
                    "Encoding (pass 2/2)"
                },
                weight: if pass == "1" { 0.4 } else { 0.6 },
                reports_progress: true,
            });
        }
        Ok(steps)
    }

    /// GIF: build an optimal palette first, then encode with it, then optionally
    /// squeeze the result with gifsicle.
    fn gif_steps(&self, tmp: &TempFiles) -> Vec<Step> {
        let (input, chain) = self.video_chain();
        let chain = chain.join(",");
        let palette = tmp.palette.to_string_lossy().into_owned();
        let a = self.anim;

        let mut palette_args = self.input_args(tmp, false);
        palette_args.extend([
            "-filter_complex".into(),
            format!(
                "{input}{chain},palettegen=max_colors={}:stats_mode=diff[v]",
                a.colors.clamp(2, 256)
            ),
            "-map".into(),
            "[v]".into(),
            "-frames:v".into(),
            "1".into(),
            "-update".into(),
            "1".into(),
            palette.clone(),
        ]);

        let mut encode_args = self.input_args(tmp, false);
        encode_args.extend([
            "-i".into(),
            palette,
            "-filter_complex".into(),
            format!(
                "{input}{chain}[x];[x][1:v]paletteuse=dither={}{}[v]",
                a.dither.filter_arg(),
                if a.rect_diff {
                    ":diff_mode=rectangle"
                } else {
                    ""
                }
            ),
            "-map".into(),
            "[v]".into(),
        ]);
        encode_args.extend(self.video_codec_args());
        encode_args.push("-an".into());
        encode_args.extend(self.output_args());

        let mut steps = vec![
            Step {
                tool: Tool::Ffmpeg,
                args: palette_args,
                label: "Building palette",
                weight: 0.35,
                reports_progress: false,
            },
            Step {
                tool: Tool::Ffmpeg,
                args: encode_args,
                label: "Encoding GIF",
                weight: 0.55,
                reports_progress: true,
            },
        ];

        if self.use_gifsicle && (a.gifsicle_optimize || a.gifsicle_lossy > 0) {
            let mut args = vec!["--batch".to_string(), "--no-warnings".to_string()];
            if a.gifsicle_optimize {
                args.push("-O3".into());
            }
            if a.gifsicle_lossy > 0 {
                args.push(format!("--lossy={}", a.gifsicle_lossy.min(200)));
            }
            args.push(self.output.to_string_lossy().into_owned());
            steps.push(Step {
                tool: Tool::Gifsicle,
                args,
                label: "Optimizing (gifsicle)",
                weight: 0.1,
                reports_progress: false,
            });
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{AnimOptions, EncoderOptions};
    use std::path::PathBuf;

    fn tmp() -> TempFiles {
        TempFiles {
            passlog: PathBuf::from("passlog"),
            palette: PathBuf::from("pal.png"),
        }
    }

    fn job(format: ClipFormat) -> ClipJob {
        ClipJob {
            anime4k: Default::default(),
            input: PathBuf::from(r"C:\anime\[Group] Show - 01.mkv"),
            start: 83.5,
            end: 93.5,
            format,
            stream_copy: false,
            quality: Quality::Balanced,
            target_mb: None,
            audio_ff_index: Some(1),
            subs: None,
            crop: None,
            max_height: None,
            src_height: Some(1080),
            max_fps: None,
            src_fps: Some(23.976),
            fps: 15,
            anim: AnimOptions::default(),
            encoder: EncoderOptions::default(),
            use_gifsicle: false,
            loop_count: 1,
            output: PathBuf::from(r"C:\out\clip.webm"),
        }
    }

    fn joined(step: &Step) -> String {
        step.args.join(" ")
    }

    #[test]
    fn stream_copy_bypasses_encoding_and_uses_selected_audio() {
        for format in [ClipFormat::Mp4, ClipFormat::Mkv] {
            let mut j = job(format);
            j.stream_copy = true;
            j.audio_ff_index = Some(3);
            j.target_mb = Some(0.001);
            j.max_height = Some(240);
            j.max_fps = Some(12);
            j.loop_count = 4;
            j.crop = Some(Crop {
                x: 2,
                y: 2,
                width: 100,
                height: 100,
            });
            j.subs = Some(SubSource::Text {
                path: "subtitles.ass".into(),
                stream: 0,
            });
            assert_eq!(j.target_video_kbps().unwrap(), None);
            let steps = j.build_steps(&tmp()).unwrap();
            assert_eq!(steps.len(), 1);
            let step = &steps[0];
            assert_eq!(step.tool, Tool::Ffmpeg);
            assert_eq!(step.label, "Copying streams");
            assert!(step.reports_progress);
            let args = joined(step);
            assert!(args.contains("-ss 83.500 -t 10.000"));
            assert!(args.contains("-map 0:v:0 -map 0:3 -c copy"));
            assert!(args.contains("-avoid_negative_ts make_zero"));
            assert!(args.contains("-sn -dn"));
            for option in ["-filter_complex", "-vf", "-pass", "-crf", "-b:v"] {
                assert!(
                    !step.args.iter().any(|arg| arg == option),
                    "{option} in Copy mode"
                );
            }
            assert_eq!(args.contains("+faststart"), format == ClipFormat::Mp4);
            assert_eq!(step.args.last().unwrap(), &j.output.to_string_lossy());
        }
    }

    #[test]
    fn stream_copy_without_audio_and_unsupported_containers() {
        let mut j = job(ClipFormat::Mkv);
        j.stream_copy = true;
        j.audio_ff_index = None;
        let args = joined(&j.build_steps(&tmp()).unwrap()[0]);
        assert!(args.contains("-map 0:v:0 -an -c copy"));
        for format in [ClipFormat::Webm, ClipFormat::Gif, ClipFormat::Webp] {
            j.format = format;
            assert!(j.build_steps(&tmp()).is_err());
        }
    }

    #[test]
    fn escapes_windows_paths() {
        assert_eq!(
            escape_filter_path(Path::new(r"C:\anime\[Group] Show, it's - 01.mkv")),
            r"C\\:/anime/\[Group\] Show\, it\\\'s - 01.mkv"
        );
    }

    #[test]
    fn webm_crf_single_pass() {
        let steps = job(ClipFormat::Webm).build_steps(&tmp()).unwrap();
        assert_eq!(steps.len(), 1);
        let a = joined(&steps[0]);
        assert!(a.contains("-ss 83.500 -t 10.000 -i"));
        assert!(a.contains("[0:v:0]format=yuv420p[v]"));
        assert!(a.contains("-c:v libvpx-vp9 -row-mt 1 -deadline good -cpu-used 2"));
        assert!(a.contains("-crf 32 -b:v 0"));
        assert!(a.contains("-map 0:1 -c:a libopus"));
        assert!(a.ends_with(r"C:\out\clip.webm"));
    }

    #[test]
    fn burned_text_subs_shift_timestamps() {
        let mut j = job(ClipFormat::Mp4);
        j.subs = Some(SubSource::Text {
            path: j.input.clone(),
            stream: 1,
        });
        j.max_height = Some(720);
        let (input, chain) = j.video_chain();
        assert_eq!(
            format!("{input}{}", chain.join(",")),
            r"[0:v:0]setpts=PTS+83.500/TB,subtitles=filename=C\\:/anime/\[Group\] Show - 01.mkv:si=1,setpts=PTS-STARTPTS,scale=-2:720:flags=lanczos,format=yuv420p"
        );
    }

    #[test]
    fn crop_goes_before_text_subs_and_after_bitmap_subs() {
        let crop = Crop {
            x: 480,
            y: 270,
            width: 960,
            height: 540,
        };
        let mut j = job(ClipFormat::Mp4);
        j.crop = Some(crop);
        j.subs = Some(SubSource::Text {
            path: PathBuf::from("a.mkv"),
            stream: 0,
        });
        let (_, chain) = j.video_chain();
        assert_eq!(chain[0], "crop=960:540:480:270");
        assert!(chain[2].starts_with("subtitles="));

        j.subs = Some(SubSource::Bitmap { ff_index: 3 });
        let (input, chain) = j.video_chain();
        assert_eq!(input, "[0:v:0][0:3]");
        assert_eq!(chain[..2], ["overlay=format=auto", "crop=960:540:480:270"]);
    }

    #[test]
    fn still_frame_args() {
        let s = StillJob {
            input: PathBuf::from("ep.mkv"),
            time: 10.0,
            fps: 24.0,
            crop: Some(Crop {
                x: 0,
                y: 0,
                width: 640,
                height: 360,
            }),
            subs: None,
            output: PathBuf::from("shot.jpg"),
        };
        let a = s.args().join(" ");
        assert!(a.contains("-ss 9.990 -i ep.mkv"));
        assert!(a.contains("[0:v:0]crop=640:360:0:0[v]"));
        assert!(a.contains("-frames:v 1"));
        assert!(a.ends_with("-q:v 2 shot.jpg"));
    }

    #[test]
    fn bitmap_subs_use_overlay() {
        let mut j = job(ClipFormat::Mkv);
        j.subs = Some(SubSource::Bitmap { ff_index: 3 });
        let steps = j.build_steps(&tmp()).unwrap();
        assert!(joined(&steps[0]).contains("[0:v:0][0:3]overlay=format=auto,format=yuv420p[v]"));
    }

    #[test]
    fn no_upscale() {
        let mut j = job(ClipFormat::Mp4);
        j.max_height = Some(1080);
        j.src_height = Some(720);
        assert!(!j.video_chain().1.iter().any(|f| f.starts_with("scale=")));
    }

    #[test]
    fn webp_gets_rgb_input() {
        let j = job(ClipFormat::Webp);
        assert_eq!(j.video_chain().1.last().unwrap(), "format=bgra");
        assert!(joined(&j.build_steps(&tmp()).unwrap()[0]).contains("-q:v 85"));
    }

    #[test]
    fn fps_cap_never_raises_rate() {
        let mut j = job(ClipFormat::Webm);
        j.max_fps = Some(12);
        assert!(j.video_chain().1.contains(&"fps=12".to_string()));
        j.max_fps = Some(24);
        assert!(!j.video_chain().1.iter().any(|f| f.starts_with("fps=")));
    }

    #[test]
    fn animation_loop_uses_bounded_loop_filter() {
        let mut j = job(ClipFormat::Gif);
        assert!(!j.video_chain().1.iter().any(|f| f.starts_with("loop=")));

        j.loop_count = 3;
        // 10 s at 15 fps = 150 frames, plus a little slack; never the 32767 maximum.
        assert!(
            j.video_chain()
                .1
                .contains(&"loop=loop=2:size=152:start=0".to_string())
        );
        assert!(!j.audio_args().iter().any(|a| a == "-af"));
    }

    #[test]
    fn video_loop_repeats_encoded_file_by_stream_copy() {
        let mut j = job(ClipFormat::Mp4);
        j.loop_count = 3;
        j.target_mb = Some(4.0);
        let steps = j.build_steps(&tmp()).unwrap();
        assert_eq!(steps.len(), 3, "two encode passes, then the repeat step");
        let encode = joined(&steps[1]);
        assert!(!encode.contains("loop="));
        assert!(encode.ends_with("passlog.single.mp4"));
        let repeat = joined(&steps[2]);
        assert!(repeat.contains("-stream_loop 2 -i passlog.single.mp4 -c copy"));
        assert!(repeat.contains("+faststart"));
        assert!(repeat.ends_with(r"C:\out\clip.webm"));
        assert!(!steps[2].reports_progress);
    }

    #[test]
    fn first_pass_skips_anime4k_audio_input() {
        let mut j = job(ClipFormat::Mkv);
        j.anime4k.enabled = true;
        j.target_mb = Some(4.0);
        let steps = j.build_steps(&tmp()).unwrap();
        let labels: Vec<_> = steps.iter().map(|s| s.label).collect();
        assert_eq!(
            labels,
            [
                "Preparing Anime4K input",
                "Upscaling (Anime4K)",
                "Analyzing (pass 1/2)",
                "Encoding (pass 2/2)"
            ]
        );
        let inputs = |step: &Step| step.args.iter().filter(|a| *a == "-i").count();
        assert_eq!(inputs(&steps[2]), 1);
        assert_eq!(inputs(&steps[3]), 2);
        assert!(joined(&steps[3]).contains("-map 1:1 -c:a libopus"));
    }

    #[test]
    fn png_stills_are_eight_bit() {
        let s = StillJob {
            input: PathBuf::from("ep.mkv"),
            time: 1.0,
            fps: 24.0,
            crop: None,
            subs: None,
            output: PathBuf::from("shot.png"),
        };
        assert!(s.args().join(" ").ends_with("-pix_fmt rgb24 shot.png"));
    }

    #[test]
    fn gif_uses_palette_pass_and_dedupe() {
        let mut j = job(ClipFormat::Gif);
        j.max_height = Some(360);
        let steps = j.build_steps(&tmp()).unwrap();
        assert_eq!(steps.len(), 2, "no gifsicle step when unavailable");
        let palette = joined(&steps[0]);
        assert!(palette.contains(
            "scale=-2:360:flags=lanczos,fps=15,mpdecimate,setparams=color_trc=iec61966-2-1,palettegen=max_colors=256:stats_mode=diff[v]"
        ));
        assert!(palette.ends_with("-update 1 pal.png"));
        assert!(!steps[0].reports_progress);

        let encode = joined(&steps[1]);
        assert!(encode.contains("-i pal.png"));
        assert!(
            encode.contains("[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle[v]")
        );
        assert!(encode.contains("-fps_mode vfr"));
        assert!(encode.contains("-an"));
        assert!(!encode.contains("libopus"));
    }

    #[test]
    fn gif_gifsicle_step() {
        let mut j = job(ClipFormat::Gif);
        j.use_gifsicle = true;
        j.anim.gifsicle_lossy = 80;
        let steps = j.build_steps(&tmp()).unwrap();
        assert_eq!(steps[2].tool, Tool::Gifsicle);
        assert_eq!(
            joined(&steps[2]),
            r"--batch --no-warnings -O3 --lossy=80 C:\out\clip.webm"
        );
    }

    #[test]
    fn nvenc_quality_and_target_size() {
        let mut j = job(ClipFormat::Mp4);
        j.encoder.hw = Some(HwEncoder::Nvenc);
        let steps = j.build_steps(&tmp()).unwrap();
        assert_eq!(steps.len(), 1);
        let a = joined(&steps[0]);
        assert!(a.contains("-c:v h264_nvenc -preset p6 -tune hq -rc vbr -cq 25 -b:v 0"));
        assert!(!a.contains("libx264"));

        j.target_mb = Some(4.0);
        let steps = j.build_steps(&tmp()).unwrap();
        assert_eq!(steps.len(), 1, "GPU encodes are single-pass");
        let a = joined(&steps[0]);
        assert!(a.contains("-maxrate"));
        assert!(!a.contains("-pass"));
    }

    #[test]
    fn qsv_uses_nv12_and_webm_ignores_gpu() {
        let mut j = job(ClipFormat::Mkv);
        j.encoder.hw = Some(HwEncoder::Qsv);
        let a = joined(&j.build_steps(&tmp()).unwrap()[0]);
        assert!(a.contains("format=nv12[v]"));
        assert!(a.contains("-c:v h264_qsv"));

        let mut w = job(ClipFormat::Webm);
        w.encoder.hw = Some(HwEncoder::Nvenc);
        assert!(joined(&w.build_steps(&tmp()).unwrap()[0]).contains("libvpx-vp9"));
    }

    #[test]
    fn target_size_is_two_pass() {
        let mut j = job(ClipFormat::Webm);
        j.target_mb = Some(4.0);
        let steps = j.build_steps(&tmp()).unwrap();
        assert_eq!(steps.len(), 2);
        let p1 = joined(&steps[0]);
        let p2 = joined(&steps[1]);
        assert!(p1.contains("-pass 1 -passlogfile passlog -an -f null"));
        assert!(!p1.contains("-crf"));
        assert!(p2.contains("-pass 2"));
        assert!(p2.contains("-b:a 96k"));
        assert!(p2.ends_with(r"C:\out\clip.webm"));
    }
}
