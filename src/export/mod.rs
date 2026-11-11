//! Clip export: job description and a background worker running ffmpeg.

pub mod anime4k;
pub mod ffmpeg;
pub mod size;

use std::io::{BufRead, BufReader, Read, Write};
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

    pub fn supports_stream_copy(self) -> bool {
        matches!(self, Self::Mp4 | Self::Mkv)
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
    /// Snapshot of edited cues, materialized in the job's scratch directory.
    Edited(Arc<[u8]>),
}

impl SubSource {
    fn prepare(subs: &mut Option<Self>, scratch: &Scratch) -> Result<()> {
        if let Some(Self::Edited(data)) = subs {
            let path = scratch.path.join("edited.ass");
            std::fs::write(&path, data)?;
            *subs = Some(Self::Text { path, stream: 0 });
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ClipJob {
    pub anime4k: anime4k::Options,
    pub input: PathBuf,
    pub start: f64,
    pub end: f64,
    pub format: ClipFormat,
    /// Preserve compressed video and audio packets, bypassing all encoding settings.
    pub stream_copy: bool,
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
    /// Repeat the clip's content this many times in the output (1 = play once).
    pub loop_count: u32,
    pub output: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Gifsicle,
    Anime4k,
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

pub struct Scratch {
    pub path: PathBuf,
}

impl Scratch {
    pub fn new(parent: &Path, id: u64) -> Result<Self> {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".weebshot-{}-{id}-{serial}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .with_context(|| format!("Could not create {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Debug)]
pub enum ExportJob {
    Clip(ClipJob),
    Image(anime4k::Job),
}

impl ExportJob {
    pub fn output(&self) -> Option<&Path> {
        match self {
            Self::Clip(job) => Some(&job.output),
            Self::Image(job) => job.output.as_deref(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub gifsicle: Option<PathBuf>,
}

pub enum JobEvent {
    Stitched {
        id: u64,
        result: Result<crate::stitch::Output, String>,
    },
    Progress {
        id: u64,
        /// `None` while a step can't report progress (e.g. building a GIF palette).
        fraction: Option<f32>,
        overall: f32,
        stage: &'static str,
    },
    Finished {
        id: u64,
        result: Result<Option<image::RgbaImage>, String>,
    },
}

enum Work {
    SavePsd {
        recipe: Arc<crate::stitch::Recipe>,
        included: Vec<bool>,
        destination: PathBuf,
    },
    Export(ExportJob),
    Stitch(crate::stitch::Job),
    Recompose {
        recipe: Arc<crate::stitch::Recipe>,
        included: Vec<bool>,
    },
}

struct Submission {
    id: u64,
    job: Work,
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
                if let Work::SavePsd {
                    recipe,
                    included,
                    destination,
                } = &sub.job
                {
                    let result = crate::stitch::save_psd(
                        &tools.ffmpeg,
                        sub.id,
                        recipe,
                        included,
                        destination,
                        &sub.cancel,
                        |f, s| report(f, Some(f), s),
                    )
                    .map(|()| None)
                    .map_err(|e| format!("{e:#}"));
                    let _ = ev_tx.send(JobEvent::Finished { id: sub.id, result });
                    ctx.request_repaint();
                    continue;
                }
                let stitch_result = match &sub.job {
                    Work::Stitch(job) => Some(crate::stitch::run(
                        &tools.ffmpeg,
                        sub.id,
                        job,
                        &sub.cancel,
                        |f, s| report(f, Some(f), s),
                    )),
                    Work::Recompose { recipe, included } => Some(crate::stitch::recompose(
                        &tools.ffmpeg,
                        sub.id,
                        recipe.clone(),
                        included.clone(),
                        &sub.cancel,
                        |f, s| report(f, Some(f), s),
                    )),
                    Work::Export(_) | Work::SavePsd { .. } => None,
                };
                if let Some(result) = stitch_result {
                    let result = result.map_err(|e| format!("{e:#}"));
                    match &result {
                        Ok(out) => log::info(
                            "stitch",
                            format!(
                                "{} × {}, {} frames in {:.1}s",
                                out.dimensions.0, out.dimensions.1, out.frames, out.elapsed
                            ),
                        ),
                        Err(e) if e == "cancelled" => log::info("stitch", "cancelled"),
                        Err(e) => log::error("stitch", e),
                    }
                    let _ = ev_tx.send(JobEvent::Stitched { id: sub.id, result });
                    ctx.request_repaint();
                    continue;
                }
                let Work::Export(job) = &sub.job else {
                    unreachable!()
                };
                let result = if sub.cancel.load(Ordering::Relaxed) {
                    Err(anyhow::anyhow!("cancelled"))
                } else {
                    match job {
                        ExportJob::Clip(clip) => {
                            run_job(&tools, sub.id, clip, &sub.cancel, report).map(|()| None)
                        }
                        ExportJob::Image(image) => image.run(&tools, sub.id, &sub.cancel, report),
                    }
                };
                let result = match result {
                    Ok(image) => {
                        if let Some(output) = job.output() {
                            log::info("export", format!("finished {}", output.display()));
                        }
                        report(1.0, Some(1.0), "Done");
                        Ok(image)
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        log::error("export", &message);
                        Err(message)
                    }
                };
                let _ = ev_tx.send(JobEvent::Finished { id: sub.id, result });
                ctx.request_repaint();
            }
        });
        Self { tx, events }
    }

    pub fn submit_psd(
        &self,
        id: u64,
        recipe: Arc<crate::stitch::Recipe>,
        included: Vec<bool>,
        destination: PathBuf,
        cancel: Arc<AtomicBool>,
    ) {
        let _ = self.tx.send(Submission {
            id,
            job: Work::SavePsd {
                recipe,
                included,
                destination,
            },
            cancel,
        });
    }

    pub fn submit_recompose(
        &self,
        id: u64,
        recipe: Arc<crate::stitch::Recipe>,
        included: Vec<bool>,
        cancel: Arc<AtomicBool>,
    ) {
        let _ = self.tx.send(Submission {
            id,
            job: Work::Recompose { recipe, included },
            cancel,
        });
    }

    pub fn submit_stitch(&self, id: u64, job: crate::stitch::Job, cancel: Arc<AtomicBool>) {
        let _ = self.tx.send(Submission {
            id,
            job: Work::Stitch(job),
            cancel,
        });
    }

    pub fn submit(&self, id: u64, job: ExportJob, cancel: Arc<AtomicBool>) {
        let _ = self.tx.send(Submission {
            id,
            job: Work::Export(job),
            cancel,
        });
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
    // Small scratch files (edited subtitles) go to the temp dir; the staging dir next
    // to the output holds the encode itself and every large intermediate: pass logs,
    // the GIF palette and above all the Anime4K FFV1 videos, which run to gigabytes
    // and would otherwise fill a RAM-backed /tmp.
    let scratch = Scratch::new(&std::env::temp_dir(), id)?;
    let parent = job
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging = Scratch::new(parent, id)?;
    let tmp = TempFiles {
        passlog: staging.path.join("pass"),
        palette: staging.path.join("palette.png"),
    };
    let mut staged = job.clone();
    SubSource::prepare(&mut staged.subs, &scratch)?;
    staged.output = staging.path.join(format!("clip.{}", job.format.ext()));
    let steps = staged.build_steps(&tmp)?;
    let duration = job.duration().max(0.001);
    run_steps(tools, duration, &steps, cancel, report)?;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    if job.output.exists() {
        bail!("Output already exists: {}", job.output.display());
    }
    std::fs::rename(&staged.output, &job.output)
        .with_context(|| format!("Could not publish {}", job.output.display()))?;
    Ok(())
}

fn run_steps(
    tools: &Tools,
    duration: f64,
    steps: &[Step],
    cancel: &AtomicBool,
    report: impl Fn(f32, Option<f32>, &'static str),
) -> Result<()> {
    let total: f32 = steps.iter().map(|s| s.weight).sum();
    let mut done = 0.0f32;
    let mut video_start = None;

    for step in steps {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let program = match step.tool {
            Tool::Ffmpeg => tools.ffmpeg.clone(),
            Tool::Anime4k => find_tool("weebshot-anime4k"),
            Tool::Gifsicle => match &tools.gifsicle {
                Some(p) => p.clone(),
                None => {
                    log::warn("export", "gifsicle not found, skipping GIF optimization");
                    continue;
                }
            },
        };
        let mut args = step.args.clone();
        if step.tool == Tool::Ffmpeg
            && let Some(index) = args.iter().position(|arg| arg == "-itsoffset")
        {
            let start: i64 =
                video_start.context("Anime4K did not report a video start timestamp")?;
            args[index + 1] = format!("{:.6}", start as f64 / 1e6);
        }
        log::info(
            "export",
            format!("$ {}", log::format_command(&program, &args)),
        );
        report(
            done / total,
            step.reports_progress.then_some(0.0),
            step.label,
        );

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut cmd);
        let mut child = cmd.spawn().with_context(|| {
            if step.tool == Tool::Anime4k {
                format!(
                    "Could not start {}. Use a packaged build with the Anime4K helper installed",
                    program.display()
                )
            } else {
                format!("Could not start {}", program.display())
            }
        })?;

        let tool_name = match step.tool {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Gifsicle => "gifsicle",
            Tool::Anime4k => "Anime4K",
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

        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = unbounded();
        let stdout_reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut stage = step.label;
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            let line = match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(line) => line,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    // A process may close stdout before it finishes.
                    if child.try_wait()?.is_some() {
                        break;
                    }
                    thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
            };
            if step.tool == Tool::Anime4k
                && let Some(start) = line.strip_prefix("video_start_us=")
            {
                video_start = start.parse().ok();
            }
            if step.tool == Tool::Anime4k
                && let Some(backend) = line.strip_prefix("backend=")
            {
                stage = match backend {
                    "gpu" => "Upscaling (Anime4K GPU)",
                    "cpu" if step.args[1] == "auto" => "Upscaling (Anime4K Auto → CPU)",
                    _ => "Upscaling (Anime4K CPU)",
                };
                log::info("Anime4K", stage);
                report(done / total, step.reports_progress.then_some(0.0), stage);
            }
            if step.reports_progress
                && let Some(us) = line.strip_prefix("out_time_us=")
                && let Ok(us) = us.trim().parse::<f64>()
            {
                let f = (us / 1e6 / duration).clamp(0.0, 1.0) as f32;
                report((done + f * step.weight) / total, Some(f), stage);
            }
        }

        let status = child.wait()?;
        let _ = stdout_reader.join();
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
    Ok(())
}

/// Render one still frame (a cropped screenshot) with ffmpeg.
pub fn run_still(ffmpeg: &Path, job: &ffmpeg::StillJob) -> Result<(), String> {
    let mut job = job.clone();
    let _scratch = if matches!(job.subs, Some(SubSource::Edited(_))) {
        let scratch = Scratch::new(&std::env::temp_dir(), 0).map_err(|error| error.to_string())?;
        SubSource::prepare(&mut job.subs, &scratch).map_err(|error| error.to_string())?;
        Some(scratch)
    } else {
        None
    };
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
    let executable = std::env::current_exe().ok();
    if name == "weebshot-anime4k" {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Source launches can reuse the native build without copying it or changing PATH.
        if executable
            .as_ref()
            .is_some_and(|exe| exe.starts_with(root.join("target")))
        {
            for directory in [
                "target/anime4k",
                "target/anime4k/Release",
                "dist/anime4k/bin",
                "dist/weebshot",
            ] {
                let candidate = root.join(directory).join(&file);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    executable
        .and_then(|exe| exe.parent().map(|d| d.join(&file)))
        .filter(|p| p.is_file())
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

/// Publish an already encoded image through a sibling scratch file. Keep the old
/// destination intact if reading or writing fails, and never follow its symlink
/// while writing. `source_video` also prevents replacing the open input itself.
pub fn save_image_copy(
    encoded: &Path,
    destination: &Path,
    source_video: &Path,
    id: u64,
) -> Result<(), String> {
    let save = || -> Result<()> {
        let input = std::fs::File::open(encoded)?;
        save_image(input, destination, source_video, id, None)
    };
    save().map_err(|e| format!("Could not save image: {e:#}"))
}

pub fn save_image_data(
    data: &[u8],
    destination: &Path,
    source_video: &Path,
    id: u64,
    cancel: &AtomicBool,
) -> Result<()> {
    save_image(data, destination, source_video, id, Some(cancel))
}

fn save_image(
    mut input: impl Read,
    destination: &Path,
    source_video: &Path,
    id: u64,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let same = destination == source_video
        || destination
            .canonicalize()
            .ok()
            .zip(source_video.canonicalize().ok())
            .is_some_and(|(a, b)| a == b);
    if same {
        bail!("Choose an image destination different from the source video");
    }
    let check_cancel = || {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            bail!("cancelled");
        }
        Ok(())
    };
    check_cancel()?;
    let scratch = destination.with_extension(format!("weebshot-{}-{id}.tmp", std::process::id()));
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    // A pre-existing scratch file belongs to another operation.
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&scratch)?;
    let _cleanup = Scratch(scratch.clone());
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel()?;
        let len = input.read(&mut buffer)?;
        if len == 0 {
            break;
        }
        output.write_all(&buffer[..len])?;
    }
    output.sync_all()?;
    drop(output);
    check_cancel()?;
    #[cfg(windows)]
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    std::fs::rename(&scratch, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PathBuf {
        PathBuf::from(std::env::var("WEEBSHOT_SAMPLE").expect("set WEEBSHOT_SAMPLE"))
    }

    pub(super) fn tools() -> Tools {
        let gifsicle = find_tool("gifsicle");
        Tools {
            ffmpeg: find_tool("ffmpeg"),
            gifsicle: tool_version(&gifsicle, "--version").ok().map(|_| gifsicle),
        }
    }

    pub(super) fn job(input: &Path, format: ClipFormat, burn: bool, name: &str) -> ClipJob {
        let out_dir = input.parent().unwrap().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        ClipJob {
            anime4k: Default::default(),
            input: input.to_path_buf(),
            start: 20.5,
            end: 24.5,
            format,
            stream_copy: false,
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
            loop_count: 1,
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

    /// Verify remuxed video/audio packets against the original source.
    /// Uses the same fixture contract as real_ffmpeg_exports.
    #[test]
    #[ignore]
    fn real_ffmpeg_stream_copy() {
        fn packets(path: &Path, stream: &str) -> Vec<String> {
            let output = Command::new(find_tool("ffprobe"))
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    stream,
                    "-show_packets",
                    "-show_entries",
                    "packet=data_hash",
                    "-show_data_hash",
                    "sha256",
                    "-of",
                    "json",
                ])
                .arg(path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            data["packets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|packet| packet["data_hash"].as_str().unwrap().to_owned())
                .collect()
        }

        let input = sample();
        let video: std::collections::HashSet<_> = packets(&input, "v:0").into_iter().collect();
        let audio: std::collections::HashSet<_> = packets(&input, "2").into_iter().collect();
        assert!(!video.is_empty() && !audio.is_empty());
        let mut id = 300;
        for format in [ClipFormat::Mp4, ClipFormat::Mkv] {
            for include_audio in [true, false] {
                id += 1;
                let name = format!(
                    "copy-{}-{include_audio}.{}",
                    std::process::id(),
                    format.ext()
                );
                let mut j = job(&input, format, false, &name);
                j.stream_copy = true;
                j.audio_ff_index = include_audio.then_some(2);
                run(&j, id);
                let copied_video = packets(&j.output, "v:0");
                assert!(!copied_video.is_empty());
                assert!(
                    copied_video.iter().all(|packet| video.contains(packet)),
                    "video packets changed in {}",
                    j.output.display()
                );
                let copied_audio = packets(&j.output, "a:0");
                assert_eq!(!copied_audio.is_empty(), include_audio);
                assert!(
                    copied_audio.iter().all(|packet| audio.contains(packet)),
                    "selected audio packets changed in {}",
                    j.output.display()
                );
            }
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
