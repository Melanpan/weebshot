use eframe::egui::{self, RichText, vec2};

use super::{App, OptionsPage};
use crate::export::{HwEncoder, X264_PRESETS, probe_hw_encoder};
use crate::fonts::icon;
use crate::keymap::{Action, Keymap, default_binding};
use crate::settings::{Step, StepUnit};
use crate::sounds::Sound;
use crate::theme::Theme;

const PAGES: [(OptionsPage, &str, &str); 5] = [
    (OptionsPage::General, icon::FOLDER, "General"),
    (OptionsPage::Playback, icon::PLAY, "Playback"),
    (OptionsPage::Export, icon::EXPORT, "Export"),
    (OptionsPage::Appearance, icon::PALETTE, "Appearance"),
    (OptionsPage::Shortcuts, icon::KEYBOARD, "Shortcuts"),
];

/// Amount + unit editor for a step setting.
fn step_editor(ui: &mut egui::Ui, id: &str, step: &mut Step) {
    ui.horizontal(|ui| {
        let (range, speed) = match step.unit {
            StepUnit::Frames => (1.0..=1000.0, 0.2),
            StepUnit::Seconds => (0.05..=600.0, 0.05),
        };
        ui.add(
            egui::DragValue::new(&mut step.amount)
                .range(range)
                .speed(speed),
        );
        egui::ComboBox::from_id_salt(id)
            .width(80.0)
            .selected_text(match step.unit {
                StepUnit::Frames => "frames",
                StepUnit::Seconds => "seconds",
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(&mut step.unit, StepUnit::Frames, "frames")
                    .changed()
                {
                    step.amount = step.amount.round().max(1.0);
                }
                ui.selectable_value(&mut step.unit, StepUnit::Seconds, "seconds");
            });
    });
}

impl App {
    pub(super) fn options_window(&mut self, ctx: &egui::Context) {
        if !self.ui.show_options {
            return;
        }
        let open = super::native_window(
            ctx,
            "options",
            "weebshot — Options",
            [640.0, 500.0],
            [480.0, 320.0],
            |ui| {
                // Side navigation + content as panels, so long text wraps to the
                // window width instead of stretching the window.
                egui::Panel::left("options_nav")
                    .resizable(false)
                    .exact_size(132.0)
                    .frame(
                        egui::Frame::new()
                            .fill(self.palette.bg1)
                            .inner_margin(egui::Margin::same(8)),
                    )
                    .show(ui, |ui| {
                        for (page, icon, label) in PAGES {
                            let selected = self.ui.options_page == page;
                            if ui
                                .add_sized(
                                    [116.0, 26.0],
                                    egui::Button::selectable(selected, format!("{icon}  {label}")),
                                )
                                .clicked()
                            {
                                self.ui.options_page = page;
                            }
                        }
                    });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(self.palette.bg1)
                            .inner_margin(egui::Margin::same(12)),
                    )
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(ui, |ui| match self.ui.options_page {
                                OptionsPage::General => self.general_page(ui),
                                OptionsPage::Playback => self.playback_page(ui),
                                OptionsPage::Export => self.export_page(ui),
                                OptionsPage::Appearance => self.appearance_page(ui),
                                OptionsPage::Shortcuts => self.shortcuts_page(ui),
                            });
                    });
            },
        );
        if !open {
            self.ui.show_options = false;
            self.ui.capturing = None;
        }
    }

    fn general_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Output");
        let s = &mut self.settings;
        let folder = s
            .out_dir
            .as_ref()
            .map_or("Same folder as the video".to_string(), |d| {
                d.display().to_string()
            });
        ui.horizontal(|ui| {
            ui.label("Save to:");
            ui.add(egui::Label::new(folder).truncate());
        });
        ui.horizontal(|ui| {
            if ui.button("Choose folder…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                s.out_dir = Some(dir);
            }
            if ui.button("Same as video").clicked() {
                s.out_dir = None;
            }
        });
        ui.add_space(8.0);
        ui.heading("File names");
        egui::Grid::new("templates").num_columns(2).show(ui, |ui| {
            ui.label("Screenshots");
            ui.text_edit_singleline(&mut s.shot_template);
            ui.end_row();
            ui.label("Clips");
            ui.text_edit_singleline(&mut s.clip_template);
            ui.end_row();
        });
        ui.weak(
            "Tokens: {name} video file name · {time} current position · {frame} frame number · \
             {start} / {end} In/Out. \"_subbed\"/\"_raw\" is appended for Both, and \" (2)\" if the file exists.",
        );
        if ui.small_button("Reset names").clicked() {
            let d = crate::settings::Settings::default();
            s.shot_template = d.shot_template;
            s.clip_template = d.clip_template;
        }
        self.sounds_section(ui);

        ui.add_space(8.0);
        ui.heading("Updates");
        ui.checkbox(
            &mut self.settings.check_updates,
            "Check GitHub for a new version once a day",
        );
        if ui.button("Check now").clicked() {
            self.updater.check(ui.ctx());
            self.ui.show_update = true;
        }
    }

    fn sounds_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.heading("Sounds");
        ui.checkbox(
            &mut self.settings.sounds,
            "Play a sound for screenshots and when exports finish or fail",
        );
        ui.add_enabled_ui(self.settings.sounds, |ui| {
            ui.horizontal(|ui| {
                ui.label("Volume");
                ui.add(
                    egui::Slider::new(&mut self.settings.sound_volume, 0.0..=100.0)
                        .show_value(false),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Test:");
                for s in Sound::ALL {
                    if ui.small_button(s.label()).clicked() {
                        self.sounds.play(s, self.settings.sound_volume);
                    }
                }
            });
        });
    }

    fn playback_page(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("Track selection");
        ui.weak("Comma-separated language codes, in order of preference. Applies to newly opened files.");
        let mut changed = false;
        egui::Grid::new("langs").num_columns(2).show(ui, |ui| {
            ui.label("Audio");
            changed |= ui.text_edit_singleline(&mut s.audio_langs).changed();
            ui.end_row();
            ui.label("Subtitles");
            changed |= ui.text_edit_singleline(&mut s.sub_langs).changed();
            ui.end_row();
        });
        if changed {
            self.player.set_langs(&s.audio_langs, &s.sub_langs);
        }

        ui.add_space(8.0);
        ui.heading("Stepping");
        ui.label("How far the step shortcuts move. Frame steps pause playback.");
        let km = &self.settings.keymap;
        let keys = |back: Action, fwd: Action| format!("{} / {}", km.text(back), km.text(fwd));
        let rows = [
            ("Short", keys(Action::FrameBack, Action::FrameForward)),
            (
                "Medium",
                keys(Action::SeekBackSmall, Action::SeekForwardSmall),
            ),
            ("Long", keys(Action::SeekBackBig, Action::SeekForwardBig)),
        ];
        let s = &mut self.settings;
        egui::Grid::new("steps")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for (i, (name, keys)) in rows.iter().enumerate() {
                    ui.label(*name);
                    ui.label(RichText::new(keys).monospace().weak());
                    let step = match i {
                        0 => &mut s.step_short,
                        1 => &mut s.step_medium,
                        _ => &mut s.step_long,
                    };
                    step_editor(ui, name, step);
                    ui.end_row();
                }
            });
        ui.weak("Change the keys under Shortcuts (they're \"Step back/forward\").");

        ui.add_space(8.0);
        ui.heading("Decoding");
        if ui
            .checkbox(&mut s.hwdec, "Hardware decoding")
            .on_hover_text("Turn off if you see green frames or glitches.")
            .changed()
        {
            self.player.set_hwdec(s.hwdec);
        }
    }

    fn export_page(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        ui.heading("Tools");
        egui::Grid::new("tools")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("ffmpeg");
                match &self.tool_status.ffmpeg {
                    Ok(v) => ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{} {v}", icon::CHECK)).color(p.ok),
                        )
                        .truncate(),
                    ),
                    Err(e) => ui.colored_label(p.error, format!("{} {e}", icon::X_CIRCLE)),
                };
                ui.end_row();
                ui.label("");
                ui.weak(self.tools.ffmpeg.display().to_string());
                ui.end_row();
                ui.label("gifsicle");
                match &self.tool_status.gifsicle {
                    Some(v) => ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{} {v}", icon::CHECK)).color(p.ok),
                        )
                        .truncate(),
                    ),
                    None => ui.colored_label(p.fg_dim, "not found (optional, for smaller GIFs)"),
                };
                ui.end_row();
            });
        let mut redetect = false;
        ui.horizontal(|ui| {
            if ui.button("Choose ffmpeg…").clicked()
                && let Some(f) = rfd::FileDialog::new()
                    .add_filter("ffmpeg", &["exe"])
                    .pick_file()
            {
                self.settings.ffmpeg_path = Some(f);
                redetect = true;
            }
            if ui.button("Choose gifsicle…").clicked()
                && let Some(f) = rfd::FileDialog::new()
                    .add_filter("gifsicle", &["exe"])
                    .pick_file()
            {
                self.settings.gifsicle_path = Some(f);
                redetect = true;
            }
            if ui.button("Auto-detect").clicked() {
                self.settings.ffmpeg_path = None;
                self.settings.gifsicle_path = None;
                redetect = true;
            }
        });
        if redetect {
            self.redetect_tools(ui.ctx());
        }

        ui.add_space(8.0);
        ui.heading("Encoders");
        let e = &mut self.settings.encoder;
        egui::Grid::new("encoders").num_columns(2).show(ui, |ui| {
            ui.label("H.264 preset (MP4/MKV)");
            egui::ComboBox::from_id_salt("x264")
                .selected_text(e.x264_preset.as_str())
                .show_ui(ui, |ui| {
                    for &preset in X264_PRESETS {
                        ui.selectable_value(&mut e.x264_preset, preset.to_string(), preset);
                    }
                });
            ui.end_row();
            ui.label("VP9 speed (WebM)");
            ui.add(egui::Slider::new(&mut e.vp9_cpu_used, 0..=5).text("0 = best, 5 = fastest"));
            ui.end_row();
        });
        ui.weak("Slower presets give smaller files at the same quality.");

        self.hardware_section(ui);
    }

    /// GPU encoding: detect what works (by actually encoding a test clip), then pick one.
    fn hardware_section(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        ui.add_space(8.0);
        ui.heading("Hardware encoding");
        ui.label(
            "Encode MP4/MKV on the graphics card: much faster, slightly bigger files at the \
             same quality. WebM, GIF and WebP always use the CPU.",
        );

        if let Some(rx) = &self.ui.hw_probe
            && let Ok(results) = rx.try_recv()
        {
            self.settings.hw_available = results
                .iter()
                .filter(|(_, r)| r.is_ok())
                .map(|(e, _)| *e)
                .collect();
            self.ui.hw_results = results;
            self.ui.hw_probe = None;
            // Drop a selection that turned out not to work.
            if self
                .settings
                .encoder
                .hw
                .is_some_and(|h| !self.settings.hw_available.contains(&h))
            {
                self.settings.encoder.hw = None;
            }
        }

        ui.horizontal(|ui| {
            let probing = self.ui.hw_probe.is_some();
            if ui
                .add_enabled(!probing, egui::Button::new("Detect GPU encoders"))
                .on_hover_text("Encodes a short test clip with each one")
                .clicked()
            {
                let (tx, rx) = crossbeam_channel::bounded(1);
                let ffmpeg = self.tools.ffmpeg.clone();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let results: Vec<_> = HwEncoder::ALL
                        .iter()
                        .map(|&e| (e, probe_hw_encoder(&ffmpeg, e)))
                        .collect();
                    let _ = tx.send(results);
                    ctx.request_repaint();
                });
                self.ui.hw_probe = Some(rx);
            }
            if probing {
                ui.spinner();
                ui.weak("testing…");
            }
        });

        if !self.ui.hw_results.is_empty() {
            egui::Grid::new("hw_results")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for (enc, result) in &self.ui.hw_results {
                        ui.label(enc.label());
                        match result {
                            Ok(()) => ui.colored_label(p.ok, format!("{} works", icon::CHECK)),
                            // ffmpeg's own message is cryptic ("AMFQueryVersion failed…");
                            // keep it for the curious in the tooltip.
                            Err(e) => ui
                                .colored_label(
                                    p.fg_dim,
                                    format!("{} not available on this PC", icon::X_CIRCLE),
                                )
                                .on_hover_text(format!("ffmpeg said: {e}")),
                        };
                        ui.end_row();
                    }
                });
        }

        let available = self.settings.hw_available.clone();
        let e = &mut self.settings.encoder;
        ui.horizontal(|ui| {
            ui.label("Use for MP4/MKV");
            let text = e.hw.map_or("Off (CPU, x264)", |h| h.label());
            egui::ComboBox::from_id_salt("hw_encoder")
                .selected_text(text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut e.hw, None, "Off (CPU, x264)");
                    for h in &available {
                        ui.selectable_value(&mut e.hw, Some(*h), h.label());
                    }
                });
        });
        if available.is_empty() {
            ui.weak("Run the detection to see what your GPU supports.");
        }
        if e.hw.is_some() {
            ui.weak(
                "Fit into file size uses a single capped-bitrate pass on the GPU, aiming a little \
                 under the limit.",
            );
        }
    }

    fn appearance_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Theme");
        let current = self.settings.theme;
        let mut chosen = None;
        egui::Grid::new("themes")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for t in Theme::ALL {
                    if ui.radio(current == t, t.label()).clicked() {
                        chosen = Some(t);
                    }
                    let pal = t.palette();
                    ui.horizontal(|ui| {
                        for c in [
                            pal.bg0,
                            pal.bg1,
                            pal.bg2,
                            pal.fg,
                            pal.accent,
                            pal.playhead,
                            pal.marker,
                            pal.subtitle,
                        ] {
                            let (r, _) =
                                ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 2.0, c);
                        }
                    });
                    ui.end_row();
                }
            });
        if let Some(t) = chosen {
            self.set_theme(ui.ctx(), t);
        }

        ui.add_space(8.0);
        ui.heading("Interface size");
        // Applied on release: rescaling mid-drag moves the slider out from under the pointer.
        let r = ui.add(
            egui::Slider::new(&mut self.settings.ui_scale, 0.75..=2.0)
                .step_by(0.05)
                .suffix("×"),
        );
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            ui.ctx().set_zoom_factor(self.settings.ui_scale);
        }
    }

    fn shortcuts_page(&mut self, ui: &mut egui::Ui) {
        // Key presses arrive in this window, not the main one.
        if let Some(action) = self.ui.capturing {
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some(crate::keymap::Binding::from_event(*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some(b) = pressed {
                self.ui.capturing = None;
                if b.key != egui::Key::Escape
                    && let Some(other) = self.settings.keymap.set(action, Some(b))
                {
                    self.info(format!("{b} moved from \"{}\"", other.label()));
                }
            }
        }
        ui.label("Click a shortcut, then press the new key combination. Esc cancels.");
        if ui.button("Reset all to defaults").clicked() {
            self.settings.keymap = Keymap::default();
            self.ui.capturing = None;
        }
        ui.add_space(4.0);
        let mut group = "";
        egui::Grid::new("shortcuts")
            .num_columns(3)
            .striped(true)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                for action in Action::ALL {
                    if action.group() != group {
                        group = action.group();
                        ui.label(RichText::new(group).strong());
                        ui.end_row();
                    }
                    ui.label(action.label());
                    let capturing = self.ui.capturing == Some(action);
                    let text = if capturing {
                        RichText::new("press a key…")
                            .italics()
                            .color(self.palette.marker)
                    } else {
                        match self.settings.keymap.get(action) {
                            Some(b) => RichText::new(b.to_string()).monospace(),
                            None => RichText::new("unbound").weak(),
                        }
                    };
                    if ui
                        .add_sized([140.0, 20.0], egui::Button::new(text))
                        .clicked()
                    {
                        self.ui.capturing = if capturing { None } else { Some(action) };
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button("Clear").clicked() {
                            self.settings.keymap.set(action, None);
                        }
                        if self.settings.keymap.get(action) != default_binding(action)
                            && ui.small_button("Default").clicked()
                        {
                            self.settings.keymap.set(action, default_binding(action));
                        }
                    });
                    ui.end_row();
                }
            });
    }
}
