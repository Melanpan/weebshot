//! Stitch presentation only; decoding and image processing belong to `crate::stitch`.
use super::App;
use crate::{
    export::JobEvent,
    fonts::icon,
    stitch::{Job, Output},
    theme::Palette,
    util::{fmt_ruler, fmt_time, time_tag, unique_path},
};
use eframe::egui;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct State {
    id: Option<u64>,
    cancel: Option<Arc<AtomicBool>>,
    request: Option<Job>,
    output: Option<Output>,
    texture: Option<egui::TextureHandle>,
    frame_textures: Vec<egui::TextureHandle>,
    frame_included: Vec<bool>,
    progress: f32,
    stage: String,
    error: Option<String>,
    open: bool,
    zoom: f32,
    pan: egui::Vec2,
}
impl State {
    pub fn progress(&self) -> Option<f32> {
        self.id.map(|_| self.progress)
    }
    pub fn cancel(&self) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct PreviewActions {
    save: bool,
    save_psd: bool,
    copy: bool,
    apply_frames: bool,
    retry: bool,
    close: bool,
}

fn tint(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(tint(color, 18))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(12.0).color(color))
        })
        .inner
}

fn details(ui: &mut egui::Ui, out: &Output) {
    ui.label(out.method.label());
    ui.set_max_width(340.0);
    ui.label(&out.warning);
    if out.dimensions.0 > 2048 || out.dimensions.1 > 2048 {
        ui.separator();
        ui.label(
            "The preview is reduced to 2048 pixels. The saved PNG retains full source detail.",
        );
    }
    if (out.pixel_aspect - 1.0).abs() > 0.001 {
        ui.separator();
        ui.label(
            "PNG preserves the source pixel aspect ratio. Some image viewers ignore this metadata.",
        );
    }
}

fn frame_picker(
    ui: &mut egui::Ui,
    out: &Output,
    included: &mut [bool],
    textures: &[egui::TextureHandle],
    p: Palette,
) {
    let count = included.iter().filter(|&&v| v).count();
    egui::CollapsingHeader::new(format!("Frames · {count} of {} included", included.len()))
        .id_salt("stitch-frame-picker")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Select all").clicked() {
                    included.fill(true);
                }
                if ui.small_button("Deselect all").clicked() {
                    included.fill(false);
                }
                if ui.small_button("Revert changes").clicked() {
                    included.copy_from_slice(&out.included);
                }
                ui.weak("Uncheck a changing pose, then apply the selection.");
            });
            egui::ScrollArea::horizontal()
                .id_salt("stitch-frame-strip")
                .max_height(116.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for (i, (frame, texture)) in
                            out.recipe.frames.iter().zip(textures).enumerate()
                        {
                            ui.vertical(|ui| {
                                ui.set_width(116.0);
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(116.0, 65.0),
                                    egui::Sense::click(),
                                );
                                ui.painter().rect_filled(rect, 4, p.bg0);
                                let size = egui::vec2(
                                    texture.size_vec2().x * out.pixel_aspect,
                                    texture.size_vec2().y,
                                );
                                let fit = (rect.width() / size.x).min(rect.height() / size.y);
                                ui.painter().image(
                                    texture.id(),
                                    egui::Rect::from_center_size(rect.center(), size * fit),
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::from_white_alpha(if included[i] {
                                        255
                                    } else {
                                        75
                                    }),
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    4,
                                    egui::Stroke::new(
                                        if included[i] { 2.0 } else { 1.0 },
                                        if included[i] { p.accent } else { p.bg3 },
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                if response.clicked() {
                                    included[i] = !included[i];
                                }
                                response.on_hover_ui(|ui| {
                                    ui.label(format!("Frame {} · {}", i + 1, fmt_time(frame.time)));
                                    ui.add(egui::Image::new(texture).max_width(300.0));
                                    ui.weak("Click to include or exclude this frame");
                                });
                                ui.checkbox(&mut included[i], fmt_ruler(frame.time, true));
                            });
                        }
                    });
                });
            if included.iter().filter(|&&v| v).count() < 2 {
                ui.colored_label(p.warn, "Select at least two frames.");
            }
        });
}

impl State {
    fn preview_ui(&mut self, ui: &mut egui::Ui, p: Palette, copy_hint: &str) -> PreviewActions {
        let mut actions = PreviewActions::default();
        let out = self.output.as_ref().unwrap();
        if let Some(error) = &self.error {
            ui.colored_label(p.error, format!("Last rebuild failed: {error}"));
        }
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} × {}",out.dimensions.0,out.dimensions.1)).size(20.0).strong().color(p.fg));
            chip(ui,"PNG",p.fg_dim).on_hover_text("Full-resolution PNG export");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                if out.method == crate::stitch::Method::LayeredMotion {
                    chip(ui, "Experimental", p.warn);
                    return;
                }
                let color=if out.confidence>=0.9 {p.ok}else{p.warn};
                chip(ui,&format!("{} {:.0}% confidence",icon::CHECK,out.confidence*100.0),color)
                    .on_hover_text("Confidence measures alignment agreement. Inspect the image for local animation and overlap seams before saving.");
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let direction = match out.direction {
                "Up" => "↑ Upward pan",
                "Down" => "↓ Downward pan",
                "Left" => "← Leftward pan",
                "Right" => "→ Rightward pan",
                "Zoom" => "Zoom",
                _ => "Diagonal pan",
            };
            for text in [format!("{} frames", out.frames), direction.into()] {
                ui.label(egui::RichText::new(text).size(12.0).color(p.fg_dim));
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} – {}",
                    fmt_ruler(out.range.0, true),
                    fmt_ruler(out.range.1, true)
                ))
                .size(12.0)
                .color(p.fg_dim),
            )
            .on_hover_text(format!(
                "Output range: {} – {}",
                fmt_time(out.range.0),
                fmt_time(out.range.1)
            ));
            ui.label(
                egui::RichText::new(format!("Processed in {:.1} s", out.elapsed))
                    .size(12.0)
                    .color(p.fg_dim),
            );
        });
        ui.add_space(6.0);
        if out.omitted_frames > 0 || out.search_limited {
            let title = match (out.omitted_frames, out.search_limited) {
                (0, true) => "Search limit reached · Pan may continue".to_string(),
                (n, true) => format!(
                    "{n} {} omitted · Pan may continue",
                    if n == 1 { "frame" } else { "frames" }
                ),
                (n, false) => format!(
                    "{n} {} omitted · Stable portion retained",
                    if n == 1 { "frame" } else { "frames" }
                ),
            };
            egui::Frame::new()
                .fill(tint(p.warn, 14))
                .stroke(egui::Stroke::new(1.0, tint(p.warn, 55)))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("{} {title}", icon::WARNING)).color(p.warn),
                    )
                    .id_salt("stitch_warnings")
                    .show(ui, |ui| details(ui, out));
                });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            let hint = if ui.available_width() < 600.0 {
                "Drag to pan"
            } else {
                "Drag to pan · Scroll to zoom"
            };
            ui.label(egui::RichText::new(hint).size(11.0).color(p.fg_dim))
                .on_hover_text(
                    "Drag the image to pan. Scroll to zoom. Fit restores the complete image.",
                );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(format!("{} Fit", icon::FIT))
                    .on_hover_text("Fit the complete image to the viewport")
                    .clicked()
                {
                    self.zoom = 1.0;
                    self.pan = egui::Vec2::ZERO;
                }
                if ui.button(icon::ZOOM_IN).on_hover_text("Zoom in").clicked() {
                    self.zoom = (self.zoom * 1.25).min(16.0);
                }
                ui.add_sized(
                    [36.0, 20.0],
                    egui::Label::new(
                        egui::RichText::new(format!("{:.1}×", self.zoom))
                            .small()
                            .monospace(),
                    ),
                )
                .on_hover_text("Zoom relative to the fitted image");
                if ui
                    .button(icon::ZOOM_OUT)
                    .on_hover_text("Zoom out")
                    .clicked()
                {
                    self.zoom = (self.zoom / 1.25).max(0.25);
                }
                ui.menu_button("Details", |ui| details(ui, out));
            });
        });
        frame_picker(ui, out, &mut self.frame_included, &self.frame_textures, p);
        let dirty = self.frame_included != out.included;
        if dirty {
            ui.colored_label(
                p.warn,
                "Frame selection changed. Apply to update the preview.",
            );
        }
        ui.add_space(3.0);
        if let Some(texture) = &self.texture {
            let height = (ui.available_height() - 48.0).max(100.0);
            egui::Frame::new()
                .fill(p.bg0)
                .corner_radius(7)
                .stroke(egui::Stroke::new(1.0, p.bg3))
                .inner_margin(8)
                .show(ui, |ui| {
                    let available =
                        egui::vec2(ui.available_width().max(16.0), (height - 18.0).max(80.0));
                    let (rect, response) = ui.allocate_exact_size(available, egui::Sense::drag());
                    if response.dragged() {
                        self.pan += response.drag_delta();
                    }
                    if response.hovered() {
                        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                        self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(0.25, 16.0);
                    }
                    let size = egui::vec2(
                        out.dimensions.0 as f32 * out.pixel_aspect,
                        out.dimensions.1 as f32,
                    );
                    let fit = (rect.width() / size.x).min(rect.height() / size.y);
                    let image = egui::Rect::from_center_size(
                        rect.center() + self.pan,
                        size * fit * self.zoom,
                    );
                    ui.painter().with_clip_rect(rect).image(
                        texture.id(),
                        image,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                });
        }
        ui.add_space(7.0);
        ui.horizontal(|ui| {
            actions.retry = ui
                .add_sized(
                    [72.0, 30.0],
                    egui::Button::new(format!("{} Retry", icon::RETRY)),
                )
                .clicked();
            actions.close = ui
                .add_sized([64.0, 30.0], egui::Button::new("Close"))
                .clicked();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let luma = (p.accent.r() as u32 * 299
                    + p.accent.g() as u32 * 587
                    + p.accent.b() as u32 * 114)
                    / 1000;
                let ink = if luma > 155 {
                    egui::Color32::from_gray(20)
                } else {
                    egui::Color32::WHITE
                };
                let text = if dirty {
                    "Apply frame selection"
                } else {
                    "Save PNG…"
                };
                let enabled = !dirty || self.frame_included.iter().filter(|&&v| v).count() >= 2;
                let clicked = ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(
                            egui::RichText::new(format!("{} {text}", icon::EXPORT))
                                .strong()
                                .color(ink),
                        )
                        .fill(p.accent)
                        .min_size(egui::vec2(if dirty { 170.0 } else { 126.0 }, 32.0)),
                    )
                    .clicked();
                if dirty {
                    actions.apply_frames = clicked;
                } else {
                    actions.save = clicked;
                    actions.save_psd = ui
                        .button("Save PSD…")
                        .on_hover_text(
                            "Save the aligned frames and editable seam masks for Photoshop",
                        )
                        .clicked();
                    actions.copy = ui
                        .button(icon::CLIPBOARD)
                        .on_hover_text(copy_hint)
                        .clicked();
                }
            });
        });
        actions
    }
}

fn preview_window<'a>(ctx: &egui::Context, open: &'a mut bool) -> egui::Window<'a> {
    let id = egui::Id::unique("stitch-preview-window");
    // `collapsible(false)` hides the toggle but egui still obeys a persisted
    // collapsed state. Older builds allowed collapsing, so restore that state
    // explicitly instead of leaving a title bar with no way to expand it.
    let mut collapse = egui::collapsing_header::CollapsingState::load_with_default_open(
        ctx,
        id.with("collapsing"),
        true,
    );
    collapse.set_open(true);
    collapse.store(ctx);
    egui::Window::new("Auto Stitch")
        .id(id)
        .open(open)
        .default_size([860.0, 720.0])
        .min_width(460.0)
        .min_height(360.0)
        .collapsible(false)
        .resizable(true)
}

impl App {
    pub(super) fn finish_stitch(&mut self) {
        self.stitch.cancel();
        // Give the worker time to reap its decoder and remove scratch files on
        // normal application exit. Queued jobs have no scratch files yet.
        let started = std::time::Instant::now();
        while self.stitch.progress().is_some()
            && started.elapsed() < std::time::Duration::from_secs(2)
        {
            if let Ok(event) = self
                .runner
                .events
                .recv_timeout(std::time::Duration::from_millis(25))
            {
                let _ = self.stitch_event(event);
            }
        }
    }
    pub(super) fn start_stitch(&mut self, ctx: &egui::Context, selection: bool) {
        if self.stitch.id.is_some() {
            self.stitch.open = true;
            return;
        }
        let Some(input) = self.player.file.clone() else {
            return self.error("Open a video first");
        };
        if self.tool_status.ffmpeg.is_err() {
            return self.error("Auto Stitch needs FFmpeg. Check Options → Export.");
        }
        let range = if self.has_range() {
            if self.in_point.is_none() || self.out_point.is_none() {
                return self.error("Set both In and Out for stitching, or clear the marks for automatic discovery.");
            }
            Some(self.clip_range())
        } else if selection {
            return self.error("Set In and Out around the camera pan first");
        } else {
            None
        };
        let job = Job {
            input,
            playhead: self.player.time,
            duration: self.player.duration,
            range,
            source_fps: self.player.fps,
            options: self.settings.stitch.clone(),
        };
        self.queue_stitch(ctx, job);
    }
    fn queue_stitch(&mut self, ctx: &egui::Context, job: Job) {
        self.stitch.cancel();
        let id = self.next_job_id;
        self.next_job_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.runner.submit_stitch(id, job.clone(), cancel.clone());
        self.stitch = State {
            id: Some(id),
            cancel: Some(cancel),
            request: Some(job),
            open: true,
            zoom: 1.0,
            stage: "Queued with exports".into(),
            ..Default::default()
        };
        ctx.request_repaint();
    }
    pub(super) fn stitch_event(&mut self, event: JobEvent) -> Option<JobEvent> {
        match event {
            JobEvent::Progress {
                id, overall, stage, ..
            } if self.stitch.id == Some(id) => {
                self.stitch.progress = overall;
                self.stitch.stage = stage.into();
                None
            }
            JobEvent::Finished { id, result } if self.stitch.id == Some(id) => {
                self.stitch.id = None;
                self.stitch.cancel = None;
                match result {
                    Ok(()) => self.info("Saved layered Photoshop PSD"),
                    Err(error) if error == "cancelled" => self.info("PSD export cancelled"),
                    Err(error) => self.error(format!("Could not save PSD: {error}")),
                }
                None
            }
            JobEvent::Stitched { id, result } if self.stitch.id == Some(id) => {
                self.stitch.id = None;
                self.stitch.cancel = None;
                match result {
                    Ok(output) if self.stitch.open => {
                        let same = self
                            .stitch
                            .output
                            .as_ref()
                            .is_some_and(|old| Arc::ptr_eq(&old.recipe, &output.recipe));
                        if !same {
                            self.stitch.frame_textures.clear();
                        }
                        self.stitch.frame_included = output.included.clone();
                        self.stitch.texture = None;
                        self.stitch.zoom = 1.0;
                        self.stitch.pan = egui::Vec2::ZERO;
                        self.stitch.output = Some(output);
                    }
                    Ok(_) => (),
                    Err(error) => {
                        self.stitch.error = Some(if error == "cancelled" {
                            "Stitch cancelled. You can retry or mark a shorter In/Out range.".into()
                        } else {
                            error
                        })
                    }
                }
                None
            }
            other => Some(other),
        }
    }
    pub(super) fn stitch_window(&mut self, ctx: &egui::Context) {
        if !self.stitch.open {
            return;
        }
        if self.stitch.texture.is_none()
            && let Some(out) = &self.stitch.output
        {
            self.stitch.texture = Some(ctx.load_texture(
                "stitch-preview",
                egui::ColorImage::from_rgba_unmultiplied(
                    [out.preview.width() as usize, out.preview.height() as usize],
                    out.preview.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            ));
        }
        if let Some(out) = &self.stitch.output
            && self.stitch.frame_textures.is_empty()
        {
            self.stitch.frame_textures = out
                .recipe
                .frames
                .iter()
                .enumerate()
                .map(|(i, frame)| {
                    let image = &frame.thumbnail;
                    ctx.load_texture(
                        format!("stitch-frame-{i}"),
                        egui::ColorImage::from_rgb(
                            [image.width() as usize, image.height() as usize],
                            image.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    )
                })
                .collect();
        }
        let mut open = true;
        let mut actions = PreviewActions::default();
        let palette = self.palette;
        let shortcut = self.settings.keymap.text(crate::keymap::Action::CopyStitch);
        let copy_hint = if shortcut.is_empty() {
            "Copy stitch image to clipboard".into()
        } else {
            format!("Copy stitch image to clipboard ({shortcut})")
        };
        let state = &mut self.stitch;
        preview_window(ctx, &mut open).show(ctx, |ui| {
            if state.id.is_some() {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(&state.stage).strong());
                });
                ui.add_space(8.0);
                ui.add(egui::ProgressBar::new(state.progress).show_percentage());
                ui.add_space(8.0);
                ui.weak("You can keep using Weebshot while this runs.");
                ui.add_space(12.0);
                if ui.button("Cancel").clicked() {
                    state.cancel();
                    state.stage = "Cancelling…".into();
                }
            } else if state.output.is_some() {
                actions = state.preview_ui(ui, palette, &copy_hint);
            } else {
                if let Some(error) = &state.error {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(tint(palette.error, 16))
                        .corner_radius(6)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.colored_label(
                                palette.error,
                                egui::RichText::new("Could not complete this stitch").strong(),
                            );
                            ui.label(error);
                            ui.add_space(4.0);
                            ui.weak("Try marking a shorter In/Out range around stable artwork.");
                        });
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    actions.retry = ui.button(format!("{} Retry", icon::RETRY)).clicked();
                    actions.close = ui.button("Close").clicked();
                });
            }
        });
        if actions.copy {
            self.copy_stitch(ctx);
        }
        if actions.save {
            self.save_stitch(ctx, false);
        }
        if actions.save_psd {
            self.save_stitch(ctx, true);
        }
        if actions.apply_frames {
            self.apply_stitch_frames(ctx);
        }
        if actions.retry
            && let Some(mut job) = self.stitch.request.clone()
        {
            // Retry the same source; take updated thresholds and an explicit current
            // range only when the source is still the video in the player.
            job.options = self.settings.stitch.clone();
            if self.player.file.as_ref() == Some(&job.input)
                && self.in_point.is_some()
                && self.out_point.is_some()
            {
                job.range = Some(self.clip_range());
            }
            self.queue_stitch(ctx, job);
        }
        if !open || actions.close {
            self.stitch.cancel();
            self.stitch.open = false;
            self.stitch.output = None;
            self.stitch.texture = None;
            self.stitch.frame_textures.clear();
            self.stitch.frame_included.clear();
        }
    }
    fn apply_stitch_frames(&mut self, ctx: &egui::Context) {
        let Some(out) = &self.stitch.output else {
            return;
        };
        let recipe = out.recipe.clone();
        let included = self.stitch.frame_included.clone();
        let id = self.next_job_id;
        self.next_job_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.runner
            .submit_recompose(id, recipe, included, cancel.clone());
        self.stitch.id = Some(id);
        self.stitch.cancel = Some(cancel);
        self.stitch.progress = 0.0;
        self.stitch.stage = "Queued with exports".into();
        self.stitch.error = None;
        ctx.request_repaint();
    }

    pub(super) fn copy_stitch(&mut self, ctx: &egui::Context) {
        if !self.stitch.open || self.stitch.id.is_some() {
            return;
        }
        let Some(output) = &self.stitch.output else {
            return;
        };
        if self.stitch.frame_included != output.included {
            return self.error("Apply frame changes before copying the stitch");
        }
        let artifact = output.artifact.clone();
        self.load_clipboard_image(ctx, move || super::clipboard::read_image(&artifact.0));
    }

    fn save_stitch(&mut self, ctx: &egui::Context, layered: bool) {
        let Some(output) = &self.stitch.output else {
            return;
        };
        if self.shot_rx.is_some() {
            return self.error("Wait for the current image save to finish");
        }
        let source = self.stitch.request.as_ref().unwrap();
        let stem = source
            .input
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let base = format!(
            "{stem}_stitch_{}-{}",
            time_tag(output.range.0),
            time_tag(output.range.1)
        );
        let dir = self
            .settings
            .out_dir
            .clone()
            .or_else(|| source.input.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        let (format, extension) = if layered {
            ("Photoshop document", "psd")
        } else {
            ("PNG image", "png")
        };
        let suggested = unique_path(&dir, &base, extension);
        let Some(path) = rfd::FileDialog::new()
            .set_directory(&dir)
            .set_file_name(suggested.file_name().unwrap_or_default().to_string_lossy())
            .add_filter(format, &[extension])
            .save_file()
        else {
            return;
        };
        let path = path.with_extension(extension);
        if layered {
            let id = self.next_job_id;
            self.next_job_id += 1;
            let cancel = Arc::new(AtomicBool::new(false));
            self.runner.submit_psd(
                id,
                output.recipe.clone(),
                output.included.clone(),
                path,
                cancel.clone(),
            );
            self.stitch.id = Some(id);
            self.stitch.cancel = Some(cancel);
            self.stitch.progress = 0.0;
            self.stitch.stage = "Queued with exports".into();
            self.stitch.error = None;
            ctx.request_repaint();
            return;
        }
        let artifact = output.artifact.clone();
        let source_video = source.input.clone();
        let id = self.next_job_id;
        self.next_job_id += 1;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = crate::export::save_image_copy(&artifact.0, &path, &source_video, id)
                .map(|()| vec![path.display().to_string()]);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.shot_rx = Some(rx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_restores_body_from_legacy_collapsed_window() {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let id = egui::Id::unique("stitch-preview-window");
        let mut open = true;
        let input = |time| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        // Reproduce the old title-bar-only state, including egui's saved area
        // and resize state. A non-collapsible window alone cannot recover it.
        let mut collapsed_rect = egui::Rect::NOTHING;
        let mut rendered = ctx.run_ui(input(0.0), |ui| {
            let response = egui::Window::new("Auto Stitch")
                .id(id)
                .default_open(false)
                .show(ui.ctx(), |_| panic!("legacy window should be collapsed"))
                .unwrap();
            collapsed_rect = response.response.rect;
        });
        rendered.textures_delta.clear(); // no GPU renderer in this regression test
        assert!(
            !egui::collapsing_header::CollapsingState::load(&ctx, id.with("collapsing"))
                .unwrap()
                .is_open()
        );
        let mut shown = false;
        let mut rect = egui::Rect::NOTHING;
        for frame in 1..=3 {
            let mut rendered = ctx.run_ui(input(frame as f64 * 0.1), |ui| {
                let response = preview_window(ui.ctx(), &mut open)
                    .show(ui.ctx(), |ui| {
                        shown = true;
                        ui.label("The preview and save controls are reachable");
                    })
                    .unwrap();
                rect = response.response.rect;
            });
            rendered.textures_delta.clear();
        }
        assert!(
            rect.width() >= 460.0 && rect.height() > collapsed_rect.height() + 10.0,
            "preview did not recover its usable size: {rect:?}"
        );
        assert!(
            shown,
            "preview body remained hidden behind a collapsed title bar"
        );
        assert!(
            egui::collapsing_header::CollapsingState::load(&ctx, id.with("collapsing"))
                .unwrap()
                .is_open()
        );
    }
}
