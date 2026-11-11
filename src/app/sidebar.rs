use eframe::egui::{self, RichText, vec2};

use super::App;
use crate::export::{AnimOptions, ClipFormat, Crop, Dither, Quality, SubMode, size};
use crate::fonts::icon;
use crate::keymap::Action;
use crate::settings::{ShotFormat, SideTab};
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

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum CaptureMode {
    #[default]
    Screenshot,
    Clip,
}

fn heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(5.0);
    ui.label(RichText::new(title).strong());
}

impl App {
    pub(super) fn side_panel(&mut self, ui: &mut egui::Ui) {
        self.ui.side_rect = ui.max_rect().expand(8.0);
        let active = self.jobs_active();
        let errors = crate::log::unseen_errors();
        let icons_inline = ui.available_width() >= 350.0;
        ui.horizontal(|ui| {
            if !icons_inline {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.spacing_mut().button_padding.x = 5.0;
            }
            for (tab, glyph, label) in [
                (SideTab::Capture, icon::CAMERA, "Capture"),
                (SideTab::Stitch, icon::LAYERS, "Stitch"),
                (SideTab::Media, icon::FILM_SLATE, "Media"),
                (SideTab::Queue, icon::QUEUE, "Queue"),
                (SideTab::Logs, icon::TERMINAL, "Logs"),
            ] {
                let text = if icons_inline {
                    format!("{glyph} {label}")
                } else {
                    format!("{glyph}\n{label}")
                };
                let text = RichText::new(text);
                let text = if tab == SideTab::Logs && errors > 0 {
                    text.color(self.palette.error)
                } else {
                    text
                };
                let hint = match tab {
                    SideTab::Queue if active > 0 => format!("{active} active exports"),
                    SideTab::Logs if errors > 0 => format!("{errors} unread errors"),
                    _ => label.to_owned(),
                };
                ui.selectable_value(&mut self.settings.side_tab, tab, text)
                    .on_hover_text(hint);
            }
        });
        ui.separator();
        let tab = self.settings.side_tab;
        match tab {
            SideTab::Queue => self.queue_tab(ui),
            SideTab::Logs => {
                crate::log::mark_seen();
                self.logs_tab(ui);
            }
            _ => {
                egui::ScrollArea::vertical()
                    .id_salt(format!("{tab:?}"))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
                        match tab {
                            SideTab::Capture => self.capture_tab(ui),
                            SideTab::Stitch => self.stitch_tab(ui),
                            SideTab::Media => self.media_tab(ui),
                            _ => unreachable!(),
                        }
                    });
            }
        }
    }

    fn capture_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.ui.capture_mode,
                CaptureMode::Screenshot,
                "Screenshot",
            );
            ui.selectable_value(&mut self.ui.capture_mode, CaptureMode::Clip, "Clip");
        });
        ui.separator();
        let (action, copy, label, ready) = match self.ui.capture_mode {
            CaptureMode::Screenshot => {
                self.screenshot_ui(ui);
                (
                    Action::Screenshot,
                    Action::CopyScreenshot,
                    "Take Screenshot",
                    self.player.file.is_some(),
                )
            }
            CaptureMode::Clip => {
                self.clip_ui(ui);
                (
                    Action::ExportClip,
                    Action::CopyClip,
                    "Export Clip",
                    self.player.file.is_some()
                        && self.tool_status.ffmpeg.is_ok()
                        && self.has_range(),
                )
            }
        };
        ui.separator();
        self.output_ui(ui);
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let shortcut = self.settings.keymap.text(copy);
                let hint = match copy {
                    Action::CopyClip => "Export and copy clip files to the clipboard",
                    _ => "Copy the current frame as an image",
                };
                let hint = if shortcut.is_empty() {
                    hint.to_owned()
                } else {
                    format!("{hint} ({shortcut})")
                };
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!("{} Copy", icon::CLIPBOARD))
                            .min_size(vec2(0.0, 28.0)),
                    )
                    .on_hover_text(hint)
                    .clicked()
                {
                    self.pending.push(copy);
                }
                self.primary_action(ui, action, label, ready);
            });
        });
    }

    fn stitch_tab(&mut self, ui: &mut egui::Ui) {
        let marked = self.in_point.is_some() && self.out_point.is_some();
        self.selection_info(ui, marked, "Set both In and Out around the camera pan.");
        ui.separator();
        heading(ui, "Stitch settings");
        self.stitch_settings(ui, false);
        ui.separator();
        self.output_ui(ui);
        ui.add_space(5.0);
        let ready = self.stitch.progress().is_some()
            || (marked && self.player.file.is_some() && self.tool_status.ffmpeg.is_ok());
        ui.horizontal(|ui| {
            self.primary_action(ui, Action::StitchSelection, "Auto Stitch Selection", ready);
        });
    }

    pub(super) fn stitch_settings(&mut self, ui: &mut egui::Ui, discovery: bool) {
        ui.weak("Method");
        let stitch = &mut self.settings.stitch;
        use crate::stitch::Method;
        egui::ComboBox::from_id_salt("stitch_method")
            .width(ui.available_width())
            .truncate()
            .selected_text(stitch.method.label())
            .show_ui(ui, |ui| {
                for method in Method::ALL {
                    ui.selectable_value(&mut stitch.method, method, method.label());
                }
            });
        let adaptive = matches!(stitch.method, Method::Automatic | Method::Translation);
        let layered_motion = stitch.method == Method::LayeredMotion;
        if layered_motion {
            ui.weak("Mark In/Out around one vertical pan.");
            ui.horizontal_wrapped(|ui| {
                ui.label("Prioritize");
                ui.selectable_value(&mut stitch.prefer_background, false, "Foreground");
                ui.selectable_value(&mut stitch.prefer_background, true, "Background");
            });
        } else if !adaptive && discovery {
            ui.weak("Mark In/Out if the search window includes other shots.");
        }
        if stitch.method == Method::TwoVerticalLayers {
            let mut custom = stitch.layer_divider_percent.is_some();
            if ui
                .checkbox(&mut custom, "Set layer divider manually")
                .changed()
            {
                stitch.layer_divider_percent = custom.then_some(50);
            }
            if let Some(divider) = &mut stitch.layer_divider_percent {
                ui.weak("Divider from left");
                ui.add(egui::Slider::new(divider, 20..=80).suffix("%"));
            }
        }
        egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
            ui.weak("Frame spacing");
            ui.add(
                egui::Slider::new(&mut stitch.selection_travel, 0.02..=0.25)
                    .step_by(0.01)
                    .custom_formatter(|value, _| format!("{:.0}%", value * 100.0))
                    .custom_parser(|text| {
                        text.trim_end_matches('%')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|n| n / 100.0)
                    }),
            )
            .on_hover_text(if layered_motion {
                "Time between selected frames. Smaller values use more frames."
            } else {
                "Minimum camera travel between selected frames as a percentage of the frame."
            });
            if !layered_motion {
                ui.weak(if adaptive {
                    "Analysis frames/second (adaptive)"
                } else {
                    "Analysis frames/second"
                });
                ui.add(egui::Slider::new(&mut stitch.sampling_fps, 2.0..=16.0));
                if discovery {
                    ui.weak("Search seconds in each direction");
                    ui.add(egui::Slider::new(&mut stitch.search_seconds, 1.0..=30.0));
                }
            }
            if adaptive {
                ui.checkbox(&mut stitch.allow_pauses, "Allow short pan pauses");
            }
        });
    }

    fn selection_info(&self, ui: &mut egui::Ui, marked: bool, hint: &str) {
        heading(ui, "Selected range");
        if marked {
            let (start, end) = self.clip_range();
            ui.monospace(format!("{} → {}", fmt_time(start), fmt_time(end)));
            ui.weak(format!("Duration  {:.2} s", (end - start).max(0.0)));
        } else {
            ui.weak(hint);
        }
    }

    fn media_tab(&mut self, ui: &mut egui::Ui) {
        let p = &self.player;
        let Some(file) = &p.file else {
            ui.weak("No media loaded");
            return;
        };
        heading(ui, "Source");
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let resolution = p
            .video_width
            .zip(p.video_height)
            .map_or("—".into(), |(w, h)| format!("{w} × {h}"));
        let fps = if p.fps > 0.0 {
            format!("{:.3} fps", p.fps)
        } else {
            "—".into()
        };
        egui::Grid::new("source_info")
            .num_columns(2)
            .min_col_width(74.0)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for (label, value, numeric) in [
                    ("Filename", name.into_owned(), false),
                    ("Resolution", resolution, true),
                    (
                        "Video codec",
                        p.video_codec.as_deref().unwrap_or("—").to_owned(),
                        false,
                    ),
                    ("Frame rate", fps, true),
                    ("Duration", fmt_time(p.duration), true),
                ]
                .into_iter()
                .chain(
                    (!p.chapters.is_empty())
                        .then(|| ("Chapters", p.chapters.len().to_string(), true)),
                ) {
                    ui.weak(label);
                    let text = RichText::new(&value);
                    let text = if numeric { text.monospace() } else { text };
                    ui.add(egui::Label::new(text).truncate())
                        .on_hover_text(value);
                    ui.end_row();
                }
            });
        ui.separator();
        heading(ui, "Tracks");
        self.tracks_ui(ui);
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
                .truncate()
                .selected_text(&current)
                .show_ui(ui, |ui| self.track_list(ui, kind))
                .response
                .on_hover_text(current);
        }
        if ui.button("Load subtitle file…").clicked() {
            self.load_subtitle_dialog();
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

    /// "Match preview (zoom)" checkbox; only meaningful while the preview is zoomed in.
    fn crop_ui(ui: &mut egui::Ui, on: &mut bool, crop: Option<Crop>, label: &str) {
        ui.add_enabled_ui(crop.is_some(), |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(on, label);
                if let Some(c) = crop {
                    ui.weak(format!("{}×{}", c.width, c.height));
                }
            });
        })
        .response
        .on_disabled_hover_text("Zoom into the preview (Ctrl+wheel or the zoom buttons) to crop")
        .on_hover_text("Export what you see in the zoomed preview instead of the whole frame");
    }

    fn primary_action(&mut self, ui: &mut egui::Ui, action: Action, label: &str, enabled: bool) {
        let shortcut = self.settings.keymap.text(action);
        let text = if shortcut.is_empty() || ui.available_width() < 290.0 {
            label.to_string()
        } else {
            format!("{label}   ({shortcut})")
        };
        let button = egui::Button::new(RichText::new(text).strong())
            .min_size(vec2(ui.available_width(), 28.0))
            .fill(self.palette.accent.gamma_multiply(0.45));
        if ui
            .add_enabled(enabled, button)
            .on_hover_text(shortcut)
            .clicked()
        {
            self.pending.push(action);
        }
    }

    fn screenshot_ui(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Screenshot");
        ui.weak("Image format");
        let has_subs = self.player.sid.is_some();
        ui.horizontal(|ui| {
            for f in ShotFormat::ALL {
                ui.selectable_value(&mut self.settings.shot_format, f, f.label());
            }
        });
        ui.weak("Subtitles");
        Self::sub_mode_ui(ui, &mut self.settings.shot_subs, has_subs);
        let crop = self.view_crop();
        Self::crop_ui(
            ui,
            &mut self.settings.shot_crop,
            crop,
            "Match preview (zoom)",
        );
    }

    fn clip_ui(&mut self, ui: &mut egui::Ui) {
        self.selection_info(
            ui,
            self.has_range(),
            "Set In/Out (I / O), or mark a subtitle line (X).",
        );
        ui.separator();
        heading(ui, "Format and quality");
        let (start, end) = self.clip_range();
        let duration = (end - start).max(0.0);

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

        ui.horizontal(|ui| {
            ui.weak("Loop");
            ui.add(egui::DragValue::new(&mut self.settings.loop_count).range(1..=20))
                .on_hover_text("Repeat the clip's content this many times in the file");
            ui.weak(if self.settings.loop_count > 1 {
                "plays"
            } else {
                "play"
            });
        });

        ui.separator();
        heading(
            ui,
            if format.has_audio() {
                "Audio and subtitles"
            } else {
                "Subtitles"
            },
        );
        if format.has_audio() {
            ui.checkbox(&mut self.settings.include_audio, "Include audio");
        }
        let has_subs = self.player.sid.is_some();
        Self::sub_mode_ui(ui, &mut self.settings.clip_subs, has_subs);
        let crop = self.view_crop();
        Self::crop_ui(
            ui,
            &mut self.settings.clip_crop,
            crop,
            "Match preview (zoom)",
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
            ui.horizontal_wrapped(|ui| {
                ui.weak("Quality");
                for q in Quality::ALL {
                    ui.selectable_value(&mut s.quality, q, q.label());
                }
            });
        }
        ui.separator();
        heading(ui, "Video");
        egui::Grid::new("clip_video").num_columns(2).show(ui, |ui| {
            ui.weak("Resolution");
            egui::ComboBox::from_id_salt("max_height")
                .selected_text(height_label(s.max_height))
                .show_ui(ui, |ui| {
                    for &h in HEIGHT_PRESETS {
                        ui.selectable_value(&mut s.max_height, h, height_label(h));
                    }
                });
            ui.end_row();
            ui.weak("Frame rate");
            egui::ComboBox::from_id_salt("max_fps")
                .selected_text(fps_label(s.max_fps))
                .show_ui(ui, |ui| {
                    for &f in FPS_PRESETS {
                        ui.selectable_value(&mut s.max_fps, f, fps_label(f));
                    }
                });
            ui.end_row();
        });
    }

    fn anim_ui(&mut self, ui: &mut egui::Ui, duration: f64) {
        let p = self.palette;
        let gifsicle = self.tool_status.gifsicle.is_some();
        let gif_open = self.ui.gif_section_open;
        let s = &mut self.settings;
        let is_gif = s.clip_format == ClipFormat::Gif;

        if is_gif {
            ui.horizontal_wrapped(|ui| {
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
                    ui.weak("Palette colors");
                    ui.add(egui::Slider::new(&mut a.colors, 16..=256));
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
                        ui.weak("Lossy compression");
                        ui.add(egui::Slider::new(&mut a.gifsicle_lossy, 0..=200))
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
            ui.horizontal_wrapped(|ui| {
                ui.weak("Quality");
                for q in Quality::ALL {
                    ui.selectable_value(&mut s.quality, q, q.label());
                }
            });
            ui.checkbox(&mut s.anim.dedupe, "Drop duplicate frames");
        }

        ui.separator();
        heading(ui, "Video");
        egui::Grid::new("animation_video")
            .num_columns(2)
            .show(ui, |ui| {
                ui.weak("Resolution");
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
                ui.end_row();
                ui.weak("Frame rate");
                ui.add(
                    egui::DragValue::new(&mut s.anim_fps)
                        .range(1..=50)
                        .suffix(" fps"),
                );
                ui.end_row();
            });

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
        heading(ui, "Output destination");
        let dir = self.out_dir();
        let path = dir.as_ref().map(|d| d.display().to_string());
        let label = if self.settings.out_dir.is_some() {
            path.as_deref().unwrap_or_default()
        } else {
            "Video folder"
        };
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.settings.out_dir.is_some(),
                        egui::Button::new(icon::RETRY),
                    )
                    .on_hover_text("Reset to the source video's folder")
                    .clicked()
                {
                    self.settings.out_dir = None;
                }
                if ui
                    .add_enabled(dir.is_some(), egui::Button::new(icon::FOLDER_OPEN))
                    .on_hover_text("Open output folder")
                    .clicked()
                    && let Some(dir) = &dir
                {
                    reveal(dir, None);
                }
                if ui
                    .button("Choose…")
                    .on_hover_text("Choose output directory")
                    .clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.settings.out_dir = Some(dir);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(label).truncate())
                        .on_hover_text(path.as_deref().unwrap_or(label));
                });
            });
        });
    }
}
