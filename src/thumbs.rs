//! Timeline thumbnails, decoded by ffmpeg in the background.
//!
//! An overview pass decodes only keyframes of the whole file (fast); when the
//! timeline is zoomed in, the visible range is decoded at the needed density.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::export::hide_console;
use crate::log;

/// Thumbnail height in pixels.
pub const THUMB_H: u32 = 72;
/// Detail thumbnails are only decoded for spans shorter than this.
const MAX_DETAIL_SPAN: f64 = 180.0;
const MAX_DETAIL_FRAMES: f64 = 120.0;
const MAX_CACHED: usize = 2500;

struct Frame {
    time: f64,
    rgba: Vec<u8>,
}

pub struct Thumbs {
    pub width: u32,
    file: PathBuf,
    ffmpeg: PathBuf,
    fps: f64,
    tx: Sender<Frame>,
    rx: Receiver<Frame>,
    /// Keyed by time in milliseconds, with the moment the frame arrived (for fading in).
    frames: BTreeMap<i64, (TextureHandle, Instant)>,
    /// Bumped to cancel the running detail decode.
    generation: Arc<AtomicU64>,
    /// Generation of the last detail decode that finished.
    finished_generation: Arc<AtomicU64>,
    overview_running: Arc<AtomicBool>,
    /// Requested range and density (seconds between thumbnails) of the last detail decode.
    detail: Option<(f64, f64, f64)>,
    want: Option<((f64, f64, f64), Instant)>,
    ctx: egui::Context,
}

impl Thumbs {
    pub fn new(
        ffmpeg: PathBuf,
        file: PathBuf,
        video_w: u32,
        video_h: u32,
        fps: f64,
        ctx: egui::Context,
    ) -> Self {
        let width =
            ((THUMB_H as f64 * video_w as f64 / video_h.max(1) as f64 / 2.0).round() as u32 * 2)
                .max(2);
        let (tx, rx) = unbounded();
        let t = Self {
            width,
            file,
            ffmpeg,
            fps,
            tx,
            rx,
            frames: BTreeMap::new(),
            generation: Arc::new(AtomicU64::new(0)),
            finished_generation: Arc::new(AtomicU64::new(0)),
            overview_running: Arc::new(AtomicBool::new(true)),
            detail: None,
            want: None,
            ctx,
        };
        // Keyframes only: one decoded picture every few seconds, without decoding the rest.
        t.spawn(&["-skip_frame", "nokey"], None, None, None);
        t
    }

    pub fn matches(&self, file: &std::path::Path) -> bool {
        self.file == file
    }

    /// Run ffmpeg and stream decoded frames to the UI. `gen` cancels the run when it changes.
    fn spawn(
        &self,
        pre_input: &[&str],
        range: Option<(f64, f64)>,
        rate: Option<String>,
        generation: Option<u64>,
    ) {
        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "info"]);
        cmd.args(pre_input);
        if let Some((a, b)) = range {
            cmd.args([
                "-ss",
                &format!("{a:.3}"),
                "-t",
                &format!("{:.3}", b - a),
                "-copyts",
            ]);
        }
        cmd.arg("-i").arg(&self.file);
        let mut vf = String::new();
        if let Some(r) = rate {
            vf += &format!("fps={r},");
        }
        vf += &format!(
            "scale={}:{}:flags=fast_bilinear,showinfo",
            self.width, THUMB_H
        );
        cmd.args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &vf])
            .args([
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut cmd);

        let frame_bytes = (self.width * THUMB_H * 4) as usize;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let current = self.generation.clone();
        let finished = self.finished_generation.clone();
        let overview_running = self.overview_running.clone();
        thread::spawn(move || {
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    log::warn("thumbs", format!("ffmpeg: {e}"));
                    return;
                }
            };
            // showinfo prints each frame's timestamp on stderr, in output order.
            let (time_tx, time_rx) = unbounded::<f64>();
            let stderr = child.stderr.take().unwrap();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if let Some(rest) = line.split("pts_time:").nth(1) {
                        let t = rest
                            .split_whitespace()
                            .next()
                            .and_then(|v| v.parse::<f64>().ok());
                        if let Some(t) = t {
                            let _ = time_tx.send(t);
                        }
                    }
                }
            });
            let mut stdout = child.stdout.take().unwrap();
            let mut buf = vec![0u8; frame_bytes];
            let mut count = 0;
            let started = Instant::now();
            loop {
                if generation.is_some_and(|g| current.load(Ordering::Relaxed) != g) {
                    let _ = child.kill();
                    break;
                }
                if stdout.read_exact(&mut buf).is_err() {
                    break;
                }
                let Ok(time) = time_rx.recv() else { break };
                count += 1;
                if tx
                    .send(Frame {
                        time,
                        rgba: buf.clone(),
                    })
                    .is_err()
                {
                    let _ = child.kill();
                    break;
                }
                if count % 8 == 0 {
                    ctx.request_repaint();
                }
            }
            let _ = child.wait();
            match generation {
                Some(g) => {
                    finished.fetch_max(g, Ordering::Relaxed);
                }
                None => overview_running.store(false, Ordering::Relaxed),
            }
            ctx.request_repaint();
            if generation.is_none() {
                log::info(
                    "thumbs",
                    format!(
                        "overview: {count} keyframes in {:.1}s",
                        started.elapsed().as_secs_f32()
                    ),
                );
            }
        });
    }

    /// Upload frames that arrived since the last call.
    pub fn poll(&mut self) {
        let size = [self.width as usize, THUMB_H as usize];
        for _ in 0..64 {
            let Ok(f) = self.rx.try_recv() else { break };
            let key = (f.time * 1000.0).round() as i64;
            let image = ColorImage::from_rgba_unmultiplied(size, &f.rgba);
            let tex = self
                .ctx
                .load_texture(format!("thumb{key}"), image, TextureOptions::LINEAR);
            self.frames.insert(key, (tex, Instant::now()));
        }
        if !self.rx.is_empty() {
            self.ctx.request_repaint();
        }
        if self.frames.len() > MAX_CACHED {
            self.evict();
        }
        // Debounced detail request: only once the view has been still for a moment.
        if let Some((want, at)) = self.want
            && at.elapsed().as_millis() > 150
        {
            self.want = None;
            self.start_detail(want);
        }
    }

    fn evict(&mut self) {
        let Some((a, b, _)) = self.detail else { return };
        let (lo, hi) = ((a * 1000.0) as i64, (b * 1000.0) as i64);
        let keep_every = (self.frames.len() / (MAX_CACHED / 2)).max(2);
        let mut i = 0;
        self.frames.retain(|&k, _| {
            i += 1;
            (lo..=hi).contains(&k) || i % keep_every == 0
        });
    }

    /// The timeline shows `[a, b]` with one thumbnail every `spacing` seconds.
    pub fn request(&mut self, a: f64, b: f64, spacing: f64) {
        if b - a > MAX_DETAIL_SPAN {
            return;
        }
        let spacing = spacing.max(1.0 / self.fps);
        if let Some((da, db, ds)) = self.detail
            && a >= da
            && b <= db
            && spacing >= ds * 0.9
        {
            return; // already decoded at this density or finer
        }
        let want = (a, b, spacing);
        if self.want.map(|(w, _)| w) != Some(want) {
            self.want = Some((want, Instant::now()));
        }
    }

    fn start_detail(&mut self, (a, b, spacing): (f64, f64, f64)) {
        // Decode a bit beyond the view so small pans don't trigger a new decode.
        let pad = (b - a) * 0.25;
        let (a, b) = ((a - pad).max(0.0), b + pad);
        // The timeline asks for whole numbers of frames per thumbnail; keep the decode on
        // exactly that grid so every thumbnail lands on its slot.
        let mut n = (spacing * self.fps).round().max(1.0);
        while (b - a) * self.fps / n > MAX_DETAIL_FRAMES * 1.5 {
            n *= 2.0;
        }
        let gen_id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.detail = Some((a, b, n / self.fps));
        let rate = (n > 1.0).then(|| format!("{:.6}/{n}", self.fps));
        self.spawn(&[], Some((a, b)), rate, Some(gen_id));
    }

    /// Whether any thumbnails are being decoded right now.
    pub fn busy(&self) -> bool {
        self.overview_running.load(Ordering::Relaxed)
            || self.want.is_some()
            || self.finished_generation.load(Ordering::Relaxed)
                < self.generation.load(Ordering::Relaxed)
    }

    /// Whether thumbnails for time `t` are still being decoded.
    pub fn loading(&self, t: f64) -> bool {
        if self.overview_running.load(Ordering::Relaxed) {
            return true;
        }
        let detail_running = self.finished_generation.load(Ordering::Relaxed)
            < self.generation.load(Ordering::Relaxed);
        let in_detail = self.detail.is_some_and(|(a, b, _)| (a..=b).contains(&t));
        (detail_running && in_detail)
            || self.want.is_some_and(|((a, b, _), _)| (a..=b).contains(&t))
    }

    /// What to draw for a thumbnail slot covering `[t, t + dt)`.
    pub fn slot(&self, t: f64, dt: f64) -> Slot<'_> {
        let ms = |t: f64| ((t + 0.002) * 1000.0).round() as i64;
        // A frame decoded inside the slot is the real thing...
        let exact = self
            .frames
            .range(ms(t)..ms(t + dt).max(ms(t) + 1))
            .next()
            .map(|(_, (tex, at))| (tex, at.elapsed().as_secs_f32()));
        // ...otherwise the last frame before it is what the video shows there.
        let before = self
            .frames
            .range(..=ms(t))
            .next_back()
            .or_else(|| self.frames.iter().next())
            .map(|(_, (tex, _))| tex);
        Slot { exact, before }
    }
}

pub struct Slot<'a> {
    /// Frame inside the slot and how many seconds ago it arrived.
    pub exact: Option<(&'a TextureHandle, f32)>,
    pub before: Option<&'a TextureHandle>,
}
