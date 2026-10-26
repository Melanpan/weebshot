//! Clip export: job description and a background worker running ffmpeg.

pub mod ffmpeg;
pub mod size;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use anyhow::{Context as _, Result, bail};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::log;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipFormat {
    Webm,
    Mp4,
    Mkv,
    Gif,
    Webp,
}

impl ClipFormat {
    pub const ALL: [Self; 5] = [Self::Webm, Self::Mp4, Self::Mkv, Self::Gif, Self::Webp];

    pub fn label(self) -> &'static str {
        match self {
            Self::Webm => "WebM (VP9 + Opus)",
            Self::Mp4 => "MP4 (H.264 + AAC)",
            Self::Mkv => "MKV (H.264 + Opus)",
            Self::Gif => "GIF",
            Self::Webp => "Animated WebP",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Webm => "WebM",
            Self::Mp4 => "MP4",
            Self::Mkv => "MKV",
            Self::Gif => "GIF",
            Self::Webp => "WebP",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Webm => "webm",
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    pub fn has_audio(self) -> bool {
        !self.is_animation()
    }

    pub fn is_animation(self) -> bool {
        matches!(self, Self::Gif | Self::Webp)
    }

    pub fn supports_target_size(self) -> bool {
        !self.is_animation()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    Small,
    Balanced,
    High,
}

impl Quality {
    pub const ALL: [Self; 3] = [Self::Small, Self::Balanced, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Balanced => "Balanced",
            Self::High => "High",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubMode {
    Off,
    Burned,
    Both,
}

impl SubMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Burned, Self::Both];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "No subs",
            Self::Burned => "Burn in",
            Self::Both => "Both",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dither {
    None,
    Bayer,
    FloydSteinberg,
    Sierra,
}

impl Dither {
    pub const ALL: [Self; 4] = [Self::None, Self::Bayer, Self::FloydSteinberg, Self::Sierra];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None (flat, smallest)",
            Self::Bayer => "Bayer (ordered, compresses well)",
            Self::FloydSteinberg => "Floyd–Steinberg (smooth)",
            Self::Sierra => "Sierra (smooth, sharp)",
        }
    }

    pub fn filter_arg(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bayer => "bayer:bayer_scale=4",
            Self::FloydSteinberg => "floyd_steinberg",
            Self::Sierra => "sierra2_4a",
        }
    }
}

/// GIF/WebP size optimizations.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimOptions {
    pub colors: u16,
    pub dither: Dither,
    /// Drop frames identical to the previous one (anime is mostly drawn on 2s/3s).
    pub dedupe: bool,
    /// Only re-encode the changed rectangle of each frame.
    pub rect_diff: bool,
    /// Run gifsicle -O3 afterwards (if available).
    pub gifsicle_optimize: bool,
    /// gifsicle --lossy amount, 0 = off.
    pub gifsicle_lossy: u32,
}

impl Default for AnimOptions {
    fn default() -> Self {
        Self::preset(Quality::Balanced)
    }
}

impl AnimOptions {
    pub fn preset(q: Quality) -> Self {
        let (colors, dither, lossy) = match q {
            Quality::Small => (128, Dither::Bayer, 80),
            Quality::Balanced => (256, Dither::Bayer, 30),
            Quality::High => (256, Dither::Sierra, 0),
        };
        Self {
            colors,
            dither,
            dedupe: true,
            rect_diff: true,
            gifsicle_optimize: true,
            gifsicle_lossy: lossy,
        }
    }
}

/// GPU H.264 encoders ffmpeg can use for MP4/MKV.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HwEncoder {
    Nvenc,
    Qsv,
    Amf,
}

impl HwEncoder {
    pub const ALL: [Self; 3] = [Self::Nvenc, Self::Qsv, Self::Amf];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nvenc => "NVIDIA NVENC",
            Self::Qsv => "Intel Quick Sync",
            Self::Amf => "AMD AMF",
        }
    }

    pub fn codec(self) -> &'static str {
        match self {
            Self::Nvenc => "h264_nvenc",
            Self::Qsv => "h264_qsv",
            Self::Amf => "h264_amf",
        }
    }
}

/// Encode half a second of black with `enc` to see whether the GPU/driver supports it.
pub fn probe_hw_encoder(ffmpeg: &Path, enc: HwEncoder) -> Result<(), String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
    ])
    .args([
        "-i",
        "color=c=black:s=256x144:r=24:d=0.5",
        "-c:v",
        enc.codec(),
    ])
    .args(["-f", "null", "-"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        // The first line names the actual problem; later ones are generic wrap-up.
        let first = err.lines().map(str::trim).find(|l| !l.is_empty());
        Err(first.unwrap_or("not supported").to_string())
    }
}

/// Encoder speed/efficiency trade-offs (Options → Export).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncoderOptions {
    pub x264_preset: String,
    pub vp9_cpu_used: u8,
    /// Use this GPU encoder for MP4/MKV instead of x264.
    pub hw: Option<HwEncoder>,
}

impl Default for EncoderOptions {
    fn default() -> Self {
        Self {
            x264_preset: "slow".into(),
            vp9_cpu_used: 2,
            hw: None,
        }
    }
}

pub const X264_PRESETS: &[&str] = &[
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
];

/// A rectangle of the source frame, in source pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Crop {
    pub fn filter(self) -> String {
        format!("crop={}:{}:{}:{}", self.width, self.height, self.x, self.y)
    }
}

#[derive(Clone, Debug)]
pub enum SubSource {
    /// ASS/SRT rendered with libass. `stream` is the index among subtitle streams of `path`.
    Text { path: PathBuf, stream: usize },
    /// PGS/VobSub overlaid from the input file's stream `ff_index`.
    Bitmap { ff_index: i64 },
}

#[derive(Clone, Debug)]
pub struct ClipJob {
    pub input: PathBuf,
    pub start: f64,
    pub end: f64,
    pub format: ClipFormat,
    pub quality: Quality,
    /// Fit the output into this many MiB (2-pass), instead of constant quality.
    pub target_mb: Option<f64>,
    pub audio_ff_index: Option<i64>,
    pub subs: Option<SubSource>,
    /// Only export this part of the frame: (x, y, width, height) in source pixels.
    pub crop: Option<Crop>,
    pub max_height: Option<u32>,
    pub src_height: Option<u32>,
    /// Frame rate cap for video formats; never raises the source rate.
    pub max_fps: Option<u32>,
    pub src_fps: Option<f64>,
    /// Frame rate for GIF/WebP.
    pub fps: u32,
    pub anim: AnimOptions,
    pub encoder: EncoderOptions,
    /// Whether gifsicle is available to post-process GIFs.
    pub use_gifsicle: bool,
    pub output: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Gifsicle,
}

/// One external process run of a job.
#[derive(Clone, Debug)]
pub struct Step {
    pub tool: Tool,
    pub args: Vec<String>,
    pub label: &'static str,
    /// Share of the job's total progress bar.
    pub weight: f32,
    /// Whether ffmpeg reports meaningful `out_time` progress for this step.
    pub reports_progress: bool,
}

/// Scratch files a job may use.
pub struct TempFiles {
    pub passlog: PathBuf,
    pub palette: PathBuf,
}

impl TempFiles {
    fn new(id: u64) -> Self {
        let base = std::env::temp_dir().join(format!("weebshot-{}-{id}", std::process::id()));
        Self {
            palette: base.with_extension("palette.png"),
            passlog: base,
        }
    }

    fn cleanup(&self) {
        // x264 writes <log>-0.log(.mbtree), libvpx writes <log>-0.log.
        for suffix in [
            "-0.log",
            "-0.log.mbtree",
            "-0.log.temp",
            "-0.log.mbtree.temp",
        ] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.passlog.display()));
        }
        let _ = std::fs::remove_file(&self.palette);
    }
}

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub gifsicle: Option<PathBuf>,
}

pub enum JobEvent {
    Progress {
        id: u64,
        /// `None` while a step can't report progress (e.g. building a GIF palette).
        fraction: Option<f32>,
        overall: f32,
        stage: &'static str,
    },
    Finished {
        id: u64,
        result: Result<(), String>,
    },
}

struct Submission {
    id: u64,
    job: ClipJob,
    cancel: Arc<AtomicBool>,
}

/// Runs export jobs one at a time on a background thread.
pub struct Runner {
    tx: Sender<Submission>,
    pub events: Receiver<JobEvent>,
}

impl Runner {
    pub fn new(tools: Tools, ctx: egui::Context) -> Self {
        let (tx, rx) = unbounded::<Submission>();
        let (ev_tx, events) = unbounded();
        thread::spawn(move || {
            for sub in rx {
                let report = |overall: f32, fraction: Option<f32>, stage: &'static str| {
                    let _ = ev_tx.send(JobEvent::Progress {
                        id: sub.id,
                        fraction,
                        overall,
                        stage,
                    });
                    ctx.request_repaint();
                };
                let result = if sub.cancel.load(Ordering::Relaxed) {
                    Err("cancelled".to_string())
                } else {
                    run_job(&tools, sub.id, &sub.job, &sub.cancel, report)
                        .map_err(|e| format!("{e:#}"))
                };
                match &result {
                    Ok(()) => log::info("export", format!("finished {}", sub.job.output.display())),
                    Err(e) => log::error("export", format!("{}: {e}", sub.job.output.display())),
                }
                let _ = ev_tx.send(JobEvent::Finished { id: sub.id, result });
                ctx.request_repaint();
            }
        });
        Self { tx, events }
    }

    pub fn submit(&self, id: u64, job: ClipJob, cancel: Arc<AtomicBool>) {
        let _ = self.tx.send(Submission { id, job, cancel });
    }
}

pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn run_job(
    tools: &Tools,
    id: u64,
    job: &ClipJob,
    cancel: &AtomicBool,
    report: impl Fn(f32, Option<f32>, &'static str),
) -> Result<()> {
    let tmp = TempFiles::new(id);
    let result = job
        .build_steps(&tmp)
        .and_then(|steps| run_steps(tools, job, &steps, cancel, report));
    tmp.cleanup();
    if result.is_err() {
        let _ = std::fs::remove_file(&job.output);
    }
    result
}

fn run_steps(
    tools: &Tools,
    job: &ClipJob,
    steps: &[Step],
    cancel: &AtomicBool,
    report: impl Fn(f32, Option<f32>, &'static str),
) -> Result<()> {
    let duration = job.duration().max(0.001);
    let total: f32 = steps.iter().map(|s| s.weight).sum();
    let mut done = 0.0f32;

    for step in steps {
        let program = match step.tool {
            Tool::Ffmpeg => tools.ffmpeg.clone(),
            Tool::Gifsicle => match &tools.gifsicle {
                Some(p) => p.clone(),
                None => {
                    log::warn("export", "gifsicle not found, skipping GIF optimization");
                    continue;
                }
            },
        };
        log::info(
            "export",
            format!("$ {}", log::format_command(&program, &step.args)),
        );
        report(
            done / total,
            step.reports_progress.then_some(0.0),
            step.label,
        );

        let mut cmd = Command::new(&program);
        cmd.args(&step.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut cmd);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("could not start {}", program.display()))?;

        let tool_name = match step.tool {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Gifsicle => "gifsicle",
        };
        let stderr = child.stderr.take().unwrap();
        let stderr_reader = thread::spawn(move || {
            let mut lines = Vec::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    log::warn(tool_name, line.clone());
                    lines.push(line);
                }
            }
            lines
        });

        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            let Ok(line) = line else { break };
            if step.reports_progress
                && let Some(us) = line.strip_prefix("out_time_us=")
                && let Ok(us) = us.trim().parse::<f64>()
            {
                let f = (us / 1e6 / duration).clamp(0.0, 1.0) as f32;
                report((done + f * step.weight) / total, Some(f), step.label);
            }
        }

        let status = child.wait()?;
        let stderr = stderr_reader.join().unwrap_or_default();
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        if !status.success() {
            let tail: Vec<&str> = stderr
                .iter()
                .rev()
                .take(3)
                .rev()
                .map(String::as_str)
                .collect();
            bail!("{tool_name} failed ({status}): {}", tail.join(" | "));
        }
        done += step.weight;
    }
    report(1.0, Some(1.0), "Done");
    Ok(())
}

/// Render one still frame (a cropped screenshot) with ffmpeg.
pub fn run_still(ffmpeg: &Path, job: &ffmpeg::StillJob) -> Result<(), String> {
    let args = job.args();
    log::info(
        "export",
        format!("$ {}", log::format_command(ffmpeg, &args)),
    );
    let mut cmd = Command::new(ffmpeg);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    let err = String::from_utf8_lossy(&out.stderr);
    for line in err.lines().filter(|l| !l.trim().is_empty()) {
        log::warn("ffmpeg", line.to_string());
    }
    if out.status.success() && job.output.exists() {
        Ok(())
    } else {
        Err(err
            .lines()
            .last()
            .unwrap_or("ffmpeg failed")
            .trim()
            .to_string())
    }
}

/// A bundled tool next to our exe, otherwise whatever is on PATH.
pub fn find_tool(name: &str) -> PathBuf {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(&file)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(file))
}

/// First line of `<tool> --version`, or an error if it can't be run.
pub fn tool_version(tool: &Path, flag: &str) -> Result<String> {
    let mut cmd = Command::new(tool);
    cmd.arg(flag).stdin(Stdio::null());
    hide_console(&mut cmd);
    let out = cmd
        .output()
        .with_context(|| format!("{} not found", tool.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().next().unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PathBuf {
        PathBuf::from(std::env::var("WEEBSHOT_SAMPLE").expect("set WEEBSHOT_SAMPLE"))
    }

    fn tools() -> Tools {
        let gifsicle = find_tool("gifsicle");
        Tools {
            ffmpeg: find_tool("ffmpeg"),
            gifsicle: tool_version(&gifsicle, "--version").ok().map(|_| gifsicle),
        }
    }

    fn job(input: &Path, format: ClipFormat, burn: bool, name: &str) -> ClipJob {
        let out_dir = input.parent().unwrap().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        ClipJob {
            input: input.to_path_buf(),
            start: 20.5,
            end: 24.5,
            format,
            quality: Quality::Small,
            target_mb: None,
            audio_ff_index: Some(2),
            subs: burn.then(|| SubSource::Text {
                path: input.to_path_buf(),
                stream: 0,
            }),
            crop: None,
            max_height: Some(480),
            src_height: Some(1080),
            max_fps: None,
            src_fps: Some(23.976),
            fps: 10,
            anim: AnimOptions::default(),
            encoder: EncoderOptions::default(),
            use_gifsicle: false,
            output: out_dir.join(name),
        }
    }

    fn run(job: &ClipJob, id: u64) -> u64 {
        run_job(&tools(), id, job, &AtomicBool::new(false), |_, _, _| {})
            .unwrap_or_else(|e| panic!("{}: {e:#}", job.output.display()));
        std::fs::metadata(&job.output).unwrap().len()
    }

    /// End-to-end encode of every format through a real ffmpeg.
    /// `WEEBSHOT_SAMPLE=<video with an ASS track as first sub stream, audio at stream 2>
    ///  cargo test -- --ignored`
    #[test]
    #[ignore]
    fn real_ffmpeg_exports() {
        let input = sample();
        let mut id = 0;
        for format in ClipFormat::ALL {
            for burn in [true, false] {
                id += 1;
                let name = format!(
                    "{id}_{}.{}",
                    if burn { "subbed" } else { "raw" },
                    format.ext()
                );
                let j = job(&input, format, burn, &name);
                println!("{name}: {} bytes", run(&j, id));
            }
        }
        // GPU encoders that work on this machine.
        for hw in HwEncoder::ALL {
            if probe_hw_encoder(&tools().ffmpeg, hw).is_err() {
                println!("{hw:?}: not available, skipped");
                continue;
            }
            for target in [None, Some(1.0)] {
                id += 1;
                let mut j = job(&input, ClipFormat::Mp4, true, &format!("{id}_{hw:?}.mp4"));
                j.encoder.hw = Some(hw);
                j.target_mb = target;
                let len = run(&j, id);
                println!("{}: {len} bytes", j.output.display());
                if target.is_some() {
                    assert!(len <= 1024 * 1024, "{hw:?} over target");
                }
            }
        }
        for format in [ClipFormat::Webm, ClipFormat::Mp4] {
            id += 1;
            let mut j = job(&input, format, true, &format!("{id}_1mb.{}", format.ext()));
            j.target_mb = Some(1.0);
            let len = run(&j, id);
            println!("{}: {len} bytes", j.output.display());
            assert!(len <= 1024 * 1024, "over target");
        }
    }

    /// Cropped exports (zoomed preview): a clip with burned subs and a still frame.
    #[test]
    #[ignore]
    fn real_ffmpeg_crop() {
        let input = sample();
        let crop = Crop {
            x: 480,
            y: 540,
            width: 960,
            height: 540,
        };
        let mut j = job(&input, ClipFormat::Webm, true, "crop.webm");
        j.crop = Some(crop);
        j.max_height = None;
        println!("crop.webm: {} bytes", run(&j, 200));
        let still = ffmpeg::StillJob {
            input: input.clone(),
            time: 21.0,
            fps: 24000.0 / 1001.0,
            crop: Some(crop),
            subs: Some(SubSource::Text {
                path: input.clone(),
                stream: 0,
            }),
            output: input.parent().unwrap().join("out").join("crop-still.png"),
        };
        run_still(&tools().ffmpeg, &still).unwrap();
        println!("crop-still.png written");
    }

    /// GIF over a longer range with all optimizations must finish in reasonable time.
    #[test]
    #[ignore]
    fn real_ffmpeg_gif_long() {
        let input = sample();
        let mut j = job(&input, ClipFormat::Gif, true, "long.gif");
        let env = |k: &str, default: f64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        j.start = env("WEEBSHOT_START", 0.0);
        j.end = env("WEEBSHOT_END", 20.0);
        j.max_height = Some(360);
        j.fps = 12;
        j.use_gifsicle = tools().gifsicle.is_some();
        let t = std::time::Instant::now();
        let len = run(&j, 100);
        println!("long.gif: {len} bytes in {:?}", t.elapsed());
    }
}
