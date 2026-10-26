use eframe::egui::{self, RichText, vec2};

use super::App;
use crate::export::{AnimOptions, ClipFormat, Dither, Quality, SubMode, size};
use crate::fonts::icon;
use crate::keymap::Action;
use crate::settings::ShotFormat;
use crate::tracks::TrackKind;
use crate::util::{fmt_time, reveal};

const SIZE_PRESETS: &[f64] = &[4.0, 8.0, 10.0, 25.0, 50.0];
const HEIGHT_PRESETS: &[Option<u32>] = &[
    None,
    Some(1080),
    Some(720),
    Some(540),
    Some(480),
    Some(360),
    Some(270),
    Some(240),
];

const FPS_PRESETS: &[Option<u32>] = &[
    None,
    Some(60),
    Some(30),
    Some(25),
    Some(24),
    Some(15),
    Some(12),
];

fn fps_label(f: Option<u32>) -> String {
    f.map_or("Original".into(), |f| format!("{f} fps"))
}

fn height_label(h: Option<u32>) -> String {
    h.map_or("Original".into(), |h| format!("{h}p"))
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .default_open(true)
        .show(ui, |ui| {
            ui.add_space(2.0);
            add(ui);
            ui.add_space(4.0);
        });
}

impl App {
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 5.0;
        section(ui, "Source", |ui| self.source_info(ui));
        section(ui, "Tracks", |ui| self.tracks_ui(ui));
        section(ui, "Screenshot", |ui| self.screenshot_ui(ui));
        section(ui, "Clip", |ui| self.clip_ui(ui));
        section(ui, "Output", |ui| self.output_ui(ui));
    }

    fn source_info(&mut self, ui: &mut egui::Ui) {
        let p = &self.player;
        let Some(file) = &p.file else {
            ui.weak("No video loaded");
            return;
        };
        egui::Grid::new("source_info")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                ui.weak("File");
                ui.add(
                    egui::Label::new(file.file_name().unwrap_or_default().to_string_lossy())
                        .truncate(),
                );
                ui.end_row();
                ui.weak("Video");
                ui.label(format!(
                    "{}×{} {} · {:.3} fps",
                    p.video_width.unwrap_or(0),
                    p.video_height.unwrap_or(0),
                    p.video_codec.as_deref().unwrap_or(""),
                    p.fps
                ));
                ui.end_row();
                ui.weak("Length");
                ui.label(fmt_time(p.duration));
                ui.end_row();
                if !p.chapters.is_empty() {
                    ui.weak("Chapters");
                    ui.label(p.chapters.len().to_string());
                    ui.end_row();
                }
            });
    }

    fn tracks_ui(&mut self, ui: &mut egui::Ui) {
        for (kind, label) in [(TrackKind::Audio, "Audio"), (TrackKind::Sub, "Subtitles")] {
            let current = self
                .player
                .current(kind)
                .map_or("None".into(), |t| t.label());
            ui.weak(label);
            egui::ComboBox::from_id_salt(label)
                .width(ui.available_width())
                .selected_text(current)
                .show_ui(ui, |ui| self.track_list(ui, kind));
        }
    }

    fn sub_mode_ui(ui: &mut egui::Ui, mode: &mut SubMode, has_subs: bool) {
        ui.add_enabled_ui(has_subs, |ui| {
            ui.horizontal(|ui| {
                for m in SubMode::ALL {
                    ui.selectable_value(mode, m, m.label());
                }
            });
        })
        .response
        .on_disabled_hover_text("Select a subtitle track first");
    }

    fn big_button(&mut self, ui: &mut egui::Ui, action: Action, label: &str, enabled: bool) {
        let shortcut = self.settings.keymap.text(action);
        let text = if shortcut.is_empty() {
            label.to_string()
        } else {
            format!("{label}   ({shortcut})")
        };
        let button = egui::Button::new(RichText::new(text).strong())
            .min_size(vec2(ui.available_width(), 30.0))
            .fill(self.palette.accent.gamma_multiply(0.45));
        if ui.add_enabled(enabled, button).clicked() {
            self.pending.push(action);
        }
    }

    fn screenshot_ui(&mut self, ui: &mut egui::Ui) {
        let has_subs = self.player.sid.is_some();
        ui.horizontal(|ui| {
            for f in ShotFormat::ALL {
                ui.selectable_value(&mut self.settings.shot_format, f, f.label());
            }
        });
        Self::sub_mode_ui(ui, &mut self.settings.shot_subs, has_subs);
        let ready = self.player.file.is_some();
        self.big_button(
            ui,
            Action::Screenshot,
            &format!("{} Screenshot", icon::CAMERA),
            ready,
        );
    }

    fn clip_ui(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let (start, end) = self.clip_range();
        let duration = (end - start).max(0.0);
        if self.has_range() {
            ui.label(
                RichText::new(format!(
                    "{} → {}  ({duration:.2}s)",
                    fmt_time(start),
                    fmt_time(end)
                ))
                .monospace()
                .color(p.marker),
            );
        } else {
            ui.colored_label(p.warn, "Set In/Out first (I / O, or X on a subtitle line)");
        }

        let s = &mut self.settings;
        egui::ComboBox::from_id_salt("clip_format")
            .width(ui.available_width())
            .selected_text(s.clip_format.label())
            .show_ui(ui, |ui| {
                for f in ClipFormat::ALL {
                    ui.selectable_value(&mut s.clip_format, f, f.label());
                }
            });
        let format = s.clip_format;

        if format.is_animation() {
            self.anim_ui(ui, duration);
        } else {
            self.video_ui(ui, duration);
        }

        let has_subs = self.player.sid.is_some();
        ui.weak("Subtitles");
        Self::sub_mode_ui(ui, &mut self.settings.clip_subs, has_subs);

        let ready = self.player.file.is_some() && self.tool_status.ffmpeg.is_ok();
        self.big_button(
            ui,
            Action::ExportClip,
            &format!("{} Export clip", icon::FILM_SLATE),
            ready,
        );
    }

    fn video_ui(&mut self, ui: &mut egui::Ui, duration: f64) {
        let p = self.palette;
        let src_height = self.player.video_height;
        let s = &mut self.settings;
        ui.checkbox(&mut s.use_target_size, "Fit into file size (2-pass)");
        if s.use_target_size {
            ui.horizontal_wrapped(|ui| {
                for &mb in SIZE_PRESETS {
                    ui.selectable_value(&mut s.target_mb, mb, format!("{mb} MB"));
                }
                ui.add(
                    egui::DragValue::new(&mut s.target_mb)
                        .range(0.5..=4000.0)
                        .speed(0.1)
                        .suffix(" MB"),
                );
            });
            if duration > 0.0 {
                let audio = if s.include_audio { 96 } else { 0 };
                match size::video_kbps(s.target_mb, duration, audio) {
                    Ok(kbps) => {
                        let mut text = format!("≈ {kbps} kbps video");
                        let current = s.max_height.or(src_height);
                        if let Some(h) =
                            size::suggested_height(kbps).filter(|h| current.is_none_or(|c| c > *h))
                        {
                            text += &format!(" — consider {h}p");
                        }
                        ui.weak(text);
                    }
                    Err(e) => {
                        ui.colored_label(p.error, format!("{e}"));
                    }
                }
            }
        } else {
            ui.horizontal(|ui| {
                ui.weak("Quality");
                for q in Quality::ALL {
                    ui.selectable_value(&mut s.quality, q, q.label());
                }
            });
        }
        ui.horizontal(|ui| {
            ui.weak("Resolution");
            egui::ComboBox::from_id_salt("max_height")
                .selected_text(height_label(s.max_height))
                .show_ui(ui, |ui| {
                    for &h in HEIGHT_PRESETS {
                        ui.selectable_value(&mut s.max_height, h, height_label(h));
                    }
                });
            ui.weak("Frame rate");
            egui::ComboBox::from_id_salt("max_fps")
                .selected_text(fps_label(s.max_fps))
                .show_ui(ui, |ui| {
                    for &f in FPS_PRESETS {
                        ui.selectable_value(&mut s.max_fps, f, fps_label(f));
                    }
                });
        });
        ui.checkbox(&mut s.include_audio, "Include audio");
    }

    fn anim_ui(&mut self, ui: &mut egui::Ui, duration: f64) {
        let p = self.palette;
        let gifsicle = self.tool_status.gifsicle.is_some();
        let gif_open = self.ui.gif_section_open;
        let s = &mut self.settings;
        let is_gif = s.clip_format == ClipFormat::Gif;

        ui.horizontal(|ui| {
            ui.weak("Size");
            let height = if is_gif {
                &mut s.anim_height
            } else {
                &mut s.webp_height
            };
            egui::ComboBox::from_id_salt("anim_height")
                .selected_text(height_label(*height))
                .show_ui(ui, |ui| {
                    for &h in HEIGHT_PRESETS {
                        ui.selectable_value(height, h, height_label(h));
                    }
                });
            ui.weak("FPS");
            ui.add(egui::DragValue::new(&mut s.anim_fps).range(1..=50));
        });

        if is_gif {
            ui.horizontal(|ui| {
                ui.weak("Preset");
                for q in Quality::ALL {
                    if ui.button(q.label()).clicked() {
                        s.anim = AnimOptions::preset(q);
                        s.anim_fps = match q {
                            Quality::Small => 12,
                            Quality::Balanced => 15,
                            Quality::High => 20,
                        };
                    }
                }
            });
            egui::CollapsingHeader::new(format!("{} GIF optimization", icon::SLIDERS))
                .default_open(false)
                .open(gif_open)
                .show(ui, |ui| {
                    let a = &mut s.anim;
                    ui.add(egui::Slider::new(&mut a.colors, 16..=256).text("colors"));
                    egui::ComboBox::from_label("dither")
                        .selected_text(a.dither.label())
                        .show_ui(ui, |ui| {
                            for d in Dither::ALL {
                                ui.selectable_value(&mut a.dither, d, d.label());
                            }
                        });
                    ui.checkbox(&mut a.dedupe, "Drop duplicate frames")
                        .on_hover_text(
                            "Anime is mostly drawn on 2s/3s; held frames become one longer frame.",
                        );
                    ui.checkbox(&mut a.rect_diff, "Only redraw changed areas");
                    ui.add_enabled_ui(gifsicle, |ui| {
                        ui.checkbox(&mut a.gifsicle_optimize, "gifsicle -O3");
                        ui.add(egui::Slider::new(&mut a.gifsicle_lossy, 0..=200).text("lossy"))
                            .on_hover_text("gifsicle --lossy: higher = smaller, noisier. 0 = off.");
                    })
                    .response
                    .on_disabled_hover_text(
                        "Put gifsicle.exe next to weebshot.exe or on PATH to enable",
                    );
                    if !gifsicle {
                        ui.weak("gifsicle not found — lossy/-O3 disabled");
                    }
                });
        } else {
            ui.horizontal(|ui| {
                ui.weak("Quality");
                for q in Quality::ALL {
                    ui.selectable_value(&mut s.quality, q, q.label());
                }
            });
            ui.checkbox(&mut s.anim.dedupe, "Drop duplicate frames");
        }

        if !is_gif {
            return;
        }
        if duration > 15.0 {
            ui.colored_label(
                p.warn,
                format!(
                    "{duration:.0}s is long for a GIF — expect a large file. WebM is far smaller."
                ),
            );
        }
        if s.anim_height.is_none_or(|h| h > 540) {
            ui.colored_label(p.warn, "Large GIF size — 360p–480p is usually plenty.");
        }
    }

    fn output_ui(&mut self, ui: &mut egui::Ui) {
        let label = match &self.settings.out_dir {
            Some(d) => d.display().to_string(),
            None => "Same folder as the video".into(),
        };
        ui.add(egui::Label::new(label).truncate());
        ui.horizontal(|ui| {
            if ui.button("Choose…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.settings.out_dir = Some(dir);
            }
            if self.settings.out_dir.is_some() && ui.button("Reset").clicked() {
                self.settings.out_dir = None;
            }
            if let Some(dir) = self.out_dir()
                && ui.button("Open folder").clicked()
            {
                reveal(&dir, None);
            }
        });
    }
}
