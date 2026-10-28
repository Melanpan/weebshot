//! Turns a [`ClipJob`] into ffmpeg (and gifsicle) command lines.

use std::path::Path;

use anyhow::Result;

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
            filters.push("setpts=PTS-STARTPTS".into());
        }
        Some(SubSource::Bitmap { ff_index }) => {
            input = format!("[0:v:0][0:{ff_index}]");
            filters.push("overlay=format=auto".into());
            filters.extend(crop.map(Crop::filter));
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
        let (input, chain) = subtitle_chain(start, self.subs.as_ref(), self.crop);
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
        let (input, mut filters) = subtitle_chain(self.start, self.subs.as_ref(), self.crop);

        if let Some(h) = self.max_height
            && self.src_height.is_none_or(|src| h < src)
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
        if self.loop_count > 1 {
            // `size` just needs to safely cover the whole (already `-ss`/`-t` trimmed)
            // clip — ffmpeg stops capturing at EOF regardless, so there's no need to
            // compute an exact frame count. Capped at the filter's own hard limit of
            // 32767 frames (~22 min at 24fps), well past what this feature is for.
            filters.push(format!(
                "loop=loop={}:size=32767:start=0",
                self.loop_count - 1
            ));
        }
        (input, filters)
    }

    fn input_args(&self) -> Vec<String> {
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
        args.push(secs(self.start));
        args.push("-t".into());
        args.push(secs(self.duration()));
        args.push("-i".into());
        args.push(self.input.to_string_lossy().into_owned());
        args
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
        let mut args = vec![
            "-map".into(),
            format!("0:{idx}"),
            "-c:a".into(),
            codec.into(),
            "-b:a".into(),
            format!("{}k", self.audio_kbps()),
            "-ac".into(),
            "2".into(),
        ];
        if self.loop_count > 1 {
            // Same idea as the video `loop` filter above: a size far larger than any
            // clip's sample count, so ffmpeg just captures everything up to EOF and
            // loops that (audio has no 32767-ish cap, unlike the video filter).
            args.push("-af".into());
            args.push(format!(
                "aloop=loop={}:size=2000000000:start=0",
                self.loop_count - 1
            ));
        }
        args
    }

    fn output_args(&self) -> Vec<String> {
        let mut args: Vec<String> =
            strings(["-map_metadata", "-1", "-map_chapters", "-1", "-sn", "-dn"]).collect();
        if self.format == ClipFormat::Mp4 {
            args.extend(strings(["-movflags", "+faststart"]));
        }
        args.push(self.output.to_string_lossy().into_owned());
        args
    }

    /// Video bitrate for target-size mode, if enabled.
    pub fn target_video_kbps(&self) -> Result<Option<u32>> {
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
        if self.format == ClipFormat::Gif {
            return Ok(self.gif_steps(tmp));
        }

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
            let mut args = with_graph(self.input_args());
            args.extend(self.hw_codec_args(hw, kbps));
            args.extend(self.audio_args());
            args.extend(self.output_args());
            return Ok(vec![Step {
                tool: Tool::Ffmpeg,
                args,
                label: "Encoding (GPU)",
                weight: 1.0,
                reports_progress: true,
            }]);
        }

        let Some(kbps) = self.target_video_kbps()? else {
            let mut args = with_graph(self.input_args());
            args.extend(self.video_codec_args());
            args.extend(self.audio_args());
            args.extend(self.output_args());
            return Ok(vec![Step {
                tool: Tool::Ffmpeg,
                args,
                label: "Encoding",
                weight: 1.0,
                reports_progress: true,
            }]);
        };

        let passlog = tmp.passlog.to_string_lossy().into_owned();
        let bitrate = format!("{kbps}k");
        let mut steps = Vec::new();
        for pass in ["1", "2"] {
            let mut args = with_graph(self.input_args());
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

        let mut palette_args = self.input_args();
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

        let mut encode_args = self.input_args();
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
            input: PathBuf::from(r"C:\anime\[Group] Show - 01.mkv"),
            start: 83.5,
            end: 93.5,
            format,
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
    fn loop_count_appends_loop_filters() {
        let mut j = job(ClipFormat::Webm);
        assert!(!j.video_chain().1.iter().any(|f| f.starts_with("loop=")));
        assert!(!j.audio_args().iter().any(|a| a.starts_with("aloop=")));

        j.loop_count = 3;
        assert!(
            j.video_chain()
                .1
                .contains(&"loop=loop=2:size=32767:start=0".to_string())
        );
        let audio = j.audio_args();
        let af_pos = audio.iter().position(|a| a == "-af").unwrap();
        assert_eq!(audio[af_pos + 1], "aloop=loop=2:size=2000000000:start=0");
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
