//! `weebshot --docs-screenshots <video> <out-dir>`: drives the UI through a few
//! scenes and saves screenshots of the app's own window for the documentation.
//! Settings are neither loaded nor saved during the run.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, Rect};

use super::{App, OptionsPage, SideTab};
use crate::export::ClipFormat;
use crate::log;
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq)]
enum Crop {
    Window,
    Timeline,
    Side,
    /// Top part of the side panel, this many points tall.
    SideTop(f32),
}

type CaptureSlot = Arc<Mutex<Option<Arc<ColorImage>>>>;

#[derive(Clone, Copy, PartialEq)]
enum Scene {
    /// Wait for the file, subtitles and thumbnails, then pick a subtitle line.
    Load,
    Main,
    Timeline,
    TimelineFrames,
    Gif,
    Queue,
    Logs,
    Options,
    Help,
    Gruvbox,
    Done,
}

impl Scene {
    fn next(self) -> Self {
        use Scene::*;
        match self {
            Load => Main,
            Main => Timeline,
            Timeline => TimelineFrames,
            TimelineFrames => Gif,
            Gif => Queue,
            Queue => Logs,
            Logs => Options,
            Options => Help,
            Help => Gruvbox,
            Gruvbox | Done => Done,
        }
    }

    fn shot(self) -> Option<(&'static str, Crop)> {
        use Scene::*;
        Some(match self {
            Main => ("main.jpg", Crop::Window),
            Timeline => ("timeline.jpg", Crop::Timeline),
            TimelineFrames => ("timeline-frames.jpg", Crop::Timeline),
            Gif => ("gif-options.jpg", Crop::Side),
            Queue => ("queue.jpg", Crop::SideTop(240.0)),
            Logs => ("logs.jpg", Crop::SideTop(420.0)),
            Options => ("options.jpg", Crop::Window),
            Help => ("help.jpg", Crop::Window),
            Gruvbox => ("theme-gruvbox.jpg", Crop::Window),
            Load | Done => return None,
        })
    }
}

pub struct Script {
    out_dir: PathBuf,
    scene: Scene,
    entered: Instant,
    prepared: bool,
    capture: Option<CaptureSlot>,
}

impl Script {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            out_dir,
            scene: Scene::Load,
            entered: Instant::now(),
            prepared: false,
            capture: None,
        }
    }
}

impl App {
    pub(super) fn run_script(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_millis(50));
        let Some(script) = &self.script else { return };
        let scene = script.scene;
        let elapsed = script.entered.elapsed();

        if scene == Scene::Done {
            log::info("docs", "screenshots done");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // 1. Set the scene up once.
        if !script.prepared {
            self.prepare_scene(ctx, scene);
            if let Some(s) = &mut self.script {
                s.prepared = true;
            }
            return;
        }

        // 2. Wait until it has settled.
        let settle = match scene {
            Scene::Load => {
                let ready = self.player.duration > 0.0
                    && self.cues.as_ref().is_some_and(|c| !c.loading())
                    && self.waveform.as_ref().is_some_and(|w| !w.loading())
                    && elapsed > Duration::from_secs(8);
                if !ready && elapsed < Duration::from_secs(60) {
                    return;
                }
                self.pick_scene_line();
                self.advance();
                return;
            }
            Scene::Queue => {
                let done = self.jobs_active() == 0 && !self.jobs.is_empty();
                if !done && elapsed < Duration::from_secs(120) {
                    return;
                }
                Duration::from_millis(800)
            }
            Scene::TimelineFrames => Duration::from_secs(4),
            _ => Duration::from_millis(1500),
        };
        if elapsed < settle {
            return;
        }

        // 3. Capture, 4. save on a later frame once the image arrives.
        let script = self.script.as_mut().unwrap();
        match &script.capture {
            None => {
                let slot: CaptureSlot = Arc::new(Mutex::new(None));
                script.capture = Some(slot.clone());
                let ctx2 = ctx.clone();
                ctx.request_screenshot(move |img| {
                    *slot.lock().unwrap() = Some(img);
                    ctx2.request_repaint();
                });
            }
            Some(slot) => {
                let Some(img) = slot.lock().unwrap().take() else {
                    return;
                };
                let (name, crop) = scene.shot().unwrap();
                let rect = match crop {
                    Crop::Window => None,
                    Crop::Timeline => Some(self.timeline_rect),
                    Crop::Side => Some(self.ui.side_rect),
                    Crop::SideTop(h) => {
                        let r = self.ui.side_rect;
                        Some(Rect::from_min_size(r.min, egui::vec2(r.width(), h)))
                    }
                };
                let path = script.out_dir.join(name);
                let ppp = ctx.pixels_per_point();
                match save(&img, rect, ppp, &path) {
                    Ok(()) => log::info("docs", format!("saved {}", path.display())),
                    Err(e) => log::error("docs", format!("{}: {e}", path.display())),
                }
                self.advance();
            }
        }
    }

    fn advance(&mut self) {
        if let Some(s) = &mut self.script {
            s.scene = s.scene.next();
            s.entered = Instant::now();
            s.prepared = false;
            s.capture = None;
        }
    }

    /// Put In/Out on a readable subtitle line about a third into the episode.
    fn pick_scene_line(&mut self) {
        let d = self.player.duration;
        let cue = self.cues.as_ref().and_then(|c| {
            c.cues
                .iter()
                .find(|c| {
                    c.start > d * 0.3 && (18..60).contains(&c.text.len()) && c.end - c.start > 1.5
                })
                .cloned()
        });
        let (a, b) = cue.map_or((d * 0.3, d * 0.3 + 3.0), |c| (c.start, c.end));
        self.set_range(a, b);
        self.player.set_pause(true);
        self.player.seek(a + (b - a) * 0.5, true);
    }

    fn prepare_scene(&mut self, ctx: &egui::Context, scene: Scene) {
        let ctx_time = self.player.time;
        let (a, b) = self.clip_range();
        self.ui.show_options = false;
        self.ui.gif_section_open = None;
        match scene {
            Scene::Load => {
                let dir = std::env::temp_dir().join("weebshot-docs-exports");
                let _ = std::fs::create_dir_all(&dir);
                self.settings.out_dir = Some(dir);
            }
            Scene::Main | Scene::Timeline => {
                self.ui.side_tab = SideTab::Inspector;
                self.settings.clip_format = ClipFormat::Webm;
                let mid = (a + b) / 2.0;
                self.timeline
                    .zoom_to(mid - 30.0, mid + 30.0, self.player.duration);
            }
            Scene::TimelineFrames => {
                self.timeline
                    .zoom_to(ctx_time - 0.25, ctx_time + 0.25, self.player.duration);
            }
            Scene::Gif => {
                self.settings.clip_format = ClipFormat::Gif;
                self.ui.gif_section_open = Some(true);
                self.ui.side_tab = SideTab::Inspector;
                let mid = (a + b) / 2.0;
                self.timeline
                    .zoom_to(mid - 30.0, mid + 30.0, self.player.duration);
            }
            Scene::Queue => {
                self.settings.clip_format = ClipFormat::Webm;
                self.export_clip();
                self.settings.clip_format = ClipFormat::Gif;
                self.export_clip();
                self.ui.side_tab = SideTab::Queue;
            }
            Scene::Logs => self.ui.side_tab = SideTab::Logs,
            Scene::Options => {
                self.ui.side_tab = SideTab::Inspector;
                self.settings.clip_format = ClipFormat::Webm;
                self.ui.show_options = true;
                self.ui.options_page = OptionsPage::Playback;
            }
            Scene::Help => {
                self.ui.show_docs = true;
                self.ui.docs_page = super::docs::page_index("Timeline & seeking");
            }
            Scene::Gruvbox => {
                self.ui.show_docs = false;
                self.ui.side_tab = SideTab::Inspector;
                self.set_theme(ctx, Theme::GruvboxDark);
            }
            Scene::Done => {}
        }
    }
}

fn save(
    img: &ColorImage,
    crop: Option<Rect>,
    ppp: f32,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    let [w, h] = img.size;
    let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    let mut full = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| anyhow::anyhow!("bad screenshot buffer"))?;
    if let Some(r) = crop.filter(|r| r.is_positive()) {
        let x = (r.min.x * ppp).max(0.0) as u32;
        let y = (r.min.y * ppp).max(0.0) as u32;
        let cw = ((r.width() * ppp) as u32).min(w as u32 - x);
        let ch = ((r.height() * ppp) as u32).min(h as u32 - y);
        full = image::imageops::crop_imm(&full, x, y, cw, ch).to_image();
    }
    let max_w = 1400;
    if full.width() > max_w {
        let nh = full.height() * max_w / full.width();
        full = image::imageops::resize(&full, max_w, nh, image::imageops::FilterType::Lanczos3);
    }
    let rgb = image::DynamicImage::ImageRgba8(full).to_rgb8();
    let file = std::fs::File::create(path)?;
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 86);
    enc.encode_image(&rgb)?;
    Ok(())
}
