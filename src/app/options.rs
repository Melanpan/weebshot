use eframe::egui::{self, RichText, vec2};

use super::settings_ui::{self as layout, Setting, code, group, row};
use super::{App, OptionsPage};
use crate::export::{HwEncoder, X264_PRESETS, probe_hw_encoder};
use crate::fonts::icon;
use crate::keymap::{Action, Binding, Keymap, default_binding};
use crate::settings::{Step, StepUnit};
use crate::sounds::Sound;
use crate::theme::Theme;

const PAGES: [(OptionsPage, &str); 6] = [
    (OptionsPage::General, icon::FOLDER),
    (OptionsPage::Playback, icon::PLAY),
    (OptionsPage::Export, icon::EXPORT),
    (OptionsPage::Appearance, icon::PALETTE),
    (OptionsPage::Shortcuts, icon::KEYBOARD),
    (OptionsPage::Advanced, icon::KEYBOARD),
];

fn step_editor(ui: &mut egui::Ui, step: &mut Step) {
    ui.horizontal(|ui| {
        let (range, speed) = match step.unit {
            StepUnit::Frames => (1.0..=1000.0, 0.2),
            StepUnit::Seconds => (0.05..=600.0, 0.05),
        };
        ui.add_sized(
            [64.0, 24.0],
            egui::DragValue::new(&mut step.amount)
                .range(range)
                .speed(speed)
                .fixed_decimals(if step.unit == StepUnit::Frames { 0 } else { 2 }),
        );
        egui::ComboBox::from_id_salt("unit")
            .width(92.0)
            .selected_text(match step.unit {
                StepUnit::Frames => "Frames",
                StepUnit::Seconds => "Seconds",
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(&mut step.unit, StepUnit::Frames, "Frames")
                    .changed()
                {
                    step.amount = step.amount.round().max(1.0);
                }
                ui.selectable_value(&mut step.unit, StepUnit::Seconds, "Seconds");
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
            "Weebshot — Settings",
            [900.0, 620.0],
            [700.0, 480.0],
            |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.spacing_mut().interact_size.y = 24.0;
                // Keep secondary text readable in every existing palette, without changing the main UI.
                ui.visuals_mut().weak_text_color =
                    Some(self.palette.fg.lerp_to_gamma(self.palette.bg1, 0.25));
                let find = self.ui.capturing.is_none()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
                if self.ui.capturing.is_none()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    if !self.ui.settings_ui.query.is_empty() {
                        self.ui.settings_ui.query.clear();
                        self.ui.settings_ui.restore_page(&mut self.ui.options_page);
                    } else if self.ui.settings_ui.conflict.is_some()
                        || self.ui.settings_ui.confirm_reset
                    {
                        self.ui.settings_ui.conflict = None;
                        self.ui.settings_ui.confirm_reset = false;
                    } else {
                        self.ui.show_options = false;
                    }
                }
                egui::Panel::left("options_nav")
                    .resizable(false)
                    .exact_size(184.0)
                    .frame(
                        egui::Frame::new()
                            .fill(self.palette.bg1)
                            .inner_margin(egui::Margin::symmetric(12, 16)),
                    )
                    .show(ui, |ui| {
                        for (page, icon) in PAGES {
                            if ui
                                .add_sized(
                                    [160.0, 32.0],
                                    egui::Button::selectable(
                                        self.ui.options_page == page,
                                        (format!("{icon}  {}", page.label()), egui::Atom::grow()),
                                    ),
                                )
                                .clicked()
                            {
                                self.ui.options_page = page;
                                self.ui.settings_ui.query.clear();
                                self.ui.settings_ui.previous_page = None;
                                self.ui.capturing = None;
                            }
                        }
                        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let was_empty = self.ui.settings_ui.query.is_empty();
                                let r = ui
                                    .add_sized(
                                        [ui.available_width(), 28.0],
                                        egui::TextEdit::singleline(&mut self.ui.settings_ui.query)
                                            .id_salt("global_settings_search")
                                            .margin(egui::Margin {
                                                left: 4,
                                                right: if was_empty { 4 } else { 28 },
                                                top: 4,
                                                bottom: 4,
                                            })
                                            .hint_text("Search settings…"),
                                    )
                                    .on_hover_text("Search all settings (Ctrl+F)");
                                if find {
                                    r.request_focus();
                                }
                                if was_empty && !self.ui.settings_ui.query.is_empty() {
                                    self.ui.settings_ui.previous_page = Some(self.ui.options_page);
                                    self.ui.capturing = None;
                                }
                                if !self.ui.settings_ui.query.is_empty() {
                                    let clear_rect = egui::Rect::from_min_max(
                                        egui::pos2(r.rect.right() - 26.0, r.rect.top() + 2.0),
                                        r.rect.right_bottom() - vec2(2.0, 2.0),
                                    );
                                    if ui
                                        .put(
                                            clear_rect,
                                            egui::Button::new(RichText::new("×").size(18.0))
                                                .frame(false),
                                        )
                                        .on_hover_text("Clear search")
                                        .clicked()
                                    {
                                        self.ui.settings_ui.query.clear();
                                        r.request_focus();
                                    }
                                }
                                if self.ui.settings_ui.query.is_empty() {
                                    self.ui.settings_ui.restore_page(&mut self.ui.options_page);
                                }
                            });
                        });
                    });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(self.palette.bg1)
                            .inner_margin(egui::Margin::same(24)),
                    )
                    .show(ui, |ui| {
                        let searching = !self.ui.settings_ui.query.trim().is_empty();
                        ui.label(
                            RichText::new(if searching {
                                "Search results"
                            } else {
                                self.ui.options_page.label()
                            })
                            .size(24.0)
                            .strong(),
                        );
                        ui.add_space(12.0);
                        egui::ScrollArea::vertical()
                            .id_salt(if searching {
                                "search"
                            } else {
                                self.ui.options_page.label()
                            })
                            .auto_shrink(false)
                            .animated(false)
                            .show(ui, |ui| {
                                ui.scope_builder(
                                    egui::UiBuilder::new().ui_stack_info(
                                        egui::UiStackInfo::default().with_tag("weebshot_settings"),
                                    ),
                                    |ui| {
                                        // A finite content width makes egui wrap descriptions and long fields.
                                        ui.set_width(ui.available_width());
                                        if searching {
                                            self.settings_results(ui);
                                        } else {
                                            match self.ui.options_page {
                                                OptionsPage::General => self.general_page(ui),
                                                OptionsPage::Playback => self.playback_page(ui),
                                                OptionsPage::Export => self.export_page(ui),
                                                OptionsPage::Appearance => self.appearance_page(ui),
                                                OptionsPage::Shortcuts => self.shortcuts_page(ui),
                                                OptionsPage::Advanced => self.mcp_page(ui),
                                            }
                                        }
                                    },
                                );
                            });
                    });
            },
        );
        if !open || !self.ui.show_options {
            self.ui.show_options = false;
            self.ui.capturing = None;
        }
    }

    fn settings_results(&mut self, ui: &mut egui::Ui) {
        let query = self.ui.settings_ui.query.clone();
        let mut count = 0;
        for (page, _) in PAGES {
            let matches: Vec<_> = Setting::ALL
                .iter()
                .copied()
                .filter(|s| s.meta().category == page && s.matches(&query))
                .collect();
            if matches.is_empty() {
                continue;
            }
            group(ui, page.label());
            for setting in matches {
                let m = setting.meta();
                if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::new(format!("{}  →", m.label)),
                    )
                    .clicked()
                {
                    self.ui
                        .settings_ui
                        .navigate(setting, &mut self.ui.options_page, ui.ctx());
                    ui.ctx().request_repaint();
                }
                ui.label(RichText::new(m.description).weak());
                ui.add_space(4.0);
                count += 1;
            }
        }
        if count == 0 {
            ui.label("No matching settings. Try a setting name, category or keyword.");
        }
    }

    fn general_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "Output");
        row(ui, Setting::Output, |ui| {
            let folder = self
                .settings
                .out_dir
                .as_ref()
                .map_or("Same folder as the video".into(), |d| {
                    d.display().to_string()
                });
            code(ui, Setting::Output.meta().label, &folder);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Choose folder…").clicked()
                    && let Some(dir) = self.file_dialog.clone().pick_folder()
                {
                    self.settings.out_dir = Some(dir);
                }
                if ui.button("Same as video").clicked() {
                    self.settings.out_dir = None;
                }
            });
        });
        layout::text(ui, Setting::ShotName, &mut self.settings.shot_template);
        layout::text(ui, Setting::ClipName, &mut self.settings.clip_template);
        ui.label(
            RichText::new(
                "Existing files are kept: filename conflicts add “ (2)”, “ (3)”, and so on.",
            )
            .weak(),
        );
        ui.collapsing("Filename tokens and naming help", |ui| {
            for (token, help) in [
                ("{name}", "Video file name"),
                ("{time}", "Current position"),
                ("{frame}", "Frame number"),
                ("{start} / {end}", "In/Out positions"),
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.code(token);
                    ui.label(help);
                });
            }
            ui.label("Both subtitle modes append _subbed / _raw.");
        });
        if ui.small_button("Reset naming templates").clicked() {
            let defaults = crate::settings::Settings::default();
            self.settings.shot_template = defaults.shot_template;
            self.settings.clip_template = defaults.clip_template;
        }
        group(ui, "Sounds");
        row(ui, Setting::Sounds, |ui| {
            ui.checkbox(&mut self.settings.sounds, "Enable sound effects");
            ui.label(RichText::new(Setting::Sounds.meta().description).weak());
        });
        row(ui, Setting::SoundVolume, |ui| {
            ui.add_enabled_ui(self.settings.sounds, |ui| {
                ui.add(
                    egui::Slider::new(&mut self.settings.sound_volume, 0.0..=100.0)
                        .text("Volume")
                        .suffix("%"),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("Preview:");
                    for sound in Sound::ALL {
                        if ui.small_button(sound.label()).clicked() {
                            self.sounds.play(sound, self.settings.sound_volume);
                        }
                    }
                });
            });
        });
        group(ui, "Updates");
        row(ui, Setting::Updates, |ui| {
            ui.checkbox(
                &mut self.settings.check_updates,
                "Check for updates automatically (once a day)",
            );
            if ui.button("Check now").clicked() {
                self.updater.check(ui.ctx());
                self.ui.show_update = true;
            }
        });
    }

    fn playback_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "Track selection");
        ui.label(RichText::new(Setting::Audio.meta().description).weak());
        let audio = layout::text(ui, Setting::Audio, &mut self.settings.audio_langs).changed();
        let subs = layout::text(ui, Setting::Subtitles, &mut self.settings.sub_langs).changed();
        if audio || subs {
            self.player
                .set_langs(&self.settings.audio_langs, &self.settings.sub_langs);
        }
        group(ui, "Stepping");
        ui.label(
            RichText::new(
                "Frame stepping pauses playback. Edit associated keys on the Shortcuts page.",
            )
            .weak(),
        );
        for (id, back, forward) in [
            (Setting::ShortStep, Action::FrameBack, Action::FrameForward),
            (
                Setting::MediumStep,
                Action::SeekBackSmall,
                Action::SeekForwardSmall,
            ),
            (
                Setting::LongStep,
                Action::SeekBackBig,
                Action::SeekForwardBig,
            ),
        ] {
            row(ui, id, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [88.0, 24.0],
                        egui::Label::new(id.meta().label).halign(egui::Align::LEFT),
                    );
                    let step = match id {
                        Setting::ShortStep => &mut self.settings.step_short,
                        Setting::MediumStep => &mut self.settings.step_medium,
                        _ => &mut self.settings.step_long,
                    };
                    step_editor(ui, step);
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{} / {}",
                                self.settings.keymap.text(back),
                                self.settings.keymap.text(forward)
                            ))
                            .monospace()
                            .weak(),
                        )
                        .wrap(),
                    );
                });
            });
        }
        group(ui, "Decoding");
        row(ui, Setting::Decode, |ui| {
            if ui
                .checkbox(&mut self.settings.hwdec, Setting::Decode.meta().label)
                .changed()
            {
                self.player.set_hwdec(self.settings.hwdec);
            }
            ui.label(RichText::new(Setting::Decode.meta().description).weak());
        });
    }

    fn export_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "External tools");
        let mut redetect = false;
        row(ui, Setting::Ffmpeg, |ui| {
            match &self.tool_status.ffmpeg {
                Ok(details) => {
                    ui.colored_label(
                        self.palette.ok,
                        format!(
                            "{} {}",
                            icon::CHECK,
                            layout::concise_version("FFmpeg", details)
                        ),
                    );
                    ui.collapsing("Executable details", |ui| {
                        code(ui, "Version information", details);
                    });
                }
                Err(error) => {
                    ui.colored_label(self.palette.error, format!("FFmpeg unavailable: {error}"));
                }
            }
            code(
                ui,
                "Executable path",
                &self.tools.ffmpeg.display().to_string(),
            );
            if ui.button("Browse FFmpeg…").clicked()
                && let Some(path) = self.file_dialog.clone().pick_file()
            {
                self.settings.ffmpeg_path = Some(path);
                redetect = true;
            }
        });
        row(ui, Setting::Gifsicle, |ui| {
            match &self.tool_status.gifsicle {
                Some(details) => {
                    ui.colored_label(
                        self.palette.ok,
                        format!(
                            "{} {}",
                            icon::CHECK,
                            layout::concise_version("Gifsicle", details)
                        ),
                    )
                    .on_hover_text(details);
                }
                None => {
                    ui.label("Gifsicle not found (optional, for smaller GIFs)");
                }
            }
            let path = self
                .tools
                .gifsicle
                .as_ref()
                .or(self.settings.gifsicle_path.as_ref())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Auto-detect".into());
            code(ui, "Executable path", &path);
            if ui.button("Browse Gifsicle…").clicked()
                && let Some(path) = self.file_dialog.clone().pick_file()
            {
                self.settings.gifsicle_path = Some(path);
                redetect = true;
            }
        });
        if ui.button("Auto-detect tools").clicked() {
            self.settings.ffmpeg_path = None;
            self.settings.gifsicle_path = None;
            redetect = true;
        }
        if redetect {
            self.redetect_tools(ui.ctx());
        }
        group(ui, "Auto Stitch");
        ui.label(RichText::new("In/Out marks take precedence over playhead discovery. Stitches use clean source frames and save as PNG.").weak());
        // Stitch settings share their implementation with the main sidebar.
        self.stitch_settings_open(ui, true, layout::targeted(ui, Setting::StitchAdvanced));
        group(ui, "Video encoding");
        row(ui, Setting::H264, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("H.264 preset (MP4/MKV)");
                egui::ComboBox::from_id_salt("x264")
                    .selected_text(&self.settings.encoder.x264_preset)
                    .show_ui(ui, |ui| {
                        for &preset in X264_PRESETS {
                            ui.selectable_value(
                                &mut self.settings.encoder.x264_preset,
                                preset.to_owned(),
                                preset,
                            );
                        }
                    });
            });
            ui.label(RichText::new(Setting::H264.meta().description).weak());
        });
        row(ui, Setting::Vp9, |ui| {
            ui.label("VP9 speed (WebM)");
            ui.add(egui::Slider::new(
                &mut self.settings.encoder.vp9_cpu_used,
                0..=5,
            ));
            ui.label(RichText::new(Setting::Vp9.meta().description).weak());
        });
        group(ui, "Hardware encoding");
        row(ui, Setting::Hardware, |ui| self.hardware_section(ui));
    }
    /// GPU encoding: detect what works (by actually encoding a test clip), then pick one.
    fn hardware_section(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
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
        ui.horizontal_wrapped(|ui| {
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
        group(ui, "Theme");
        row(ui, Setting::Theme, |ui| {
            let mut chosen = self.settings.theme;
            for theme in Theme::ALL {
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(&mut chosen, theme, theme.label());
                    let p = theme.palette();
                    for color in [p.bg0, p.bg2, p.fg, p.accent, p.marker] {
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, color);
                    }
                });
            }
            if chosen != self.settings.theme {
                self.set_theme(ui.ctx(), chosen);
            }
        });
        group(ui, "Interface");
        row(ui, Setting::Scale, |ui| {
            ui.label("Interface size");
            // Keep the existing stored scale and apply-on-release semantics.
            let r = ui.add(
                egui::Slider::new(&mut self.settings.ui_scale, 0.75..=2.0)
                    .step_by(0.05)
                    .custom_formatter(|value, _| format!("{:.0}%", value * 100.0))
                    .custom_parser(|text| {
                        text.trim_end_matches('%')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|n| n / 100.0)
                    }),
            );
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                ui.ctx().set_zoom_factor(self.settings.ui_scale);
            }
        });
    }

    /// Stage conflicting changes. The keymap itself retains its existing semantics.
    fn propose_binding(&mut self, action: Action, binding: Option<Binding>) {
        self.ui.capturing = None;
        self.ui
            .settings_ui
            .propose_binding(&mut self.settings.keymap, action, binding);
    }

    fn shortcuts_page(&mut self, ui: &mut egui::Ui) {
        row(ui, Setting::Bindings, |ui| {
            if let Some(action) = self.ui.capturing {
                let pressed = ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            repeat: false,
                            modifiers,
                            ..
                        } => Some(Binding::from_event(*key, *modifiers)),
                        _ => None,
                    })
                });
                if let Some(binding) = pressed {
                    if binding.key == egui::Key::Escape {
                        self.ui.capturing = None;
                    } else {
                        self.propose_binding(action, Some(binding));
                    }
                }
            }
            if let Some(action) = self.ui.capturing {
                ui.label(
                    RichText::new(format!(
                        "Recording “{}”: press a key combination. Esc cancels.",
                        action.label()
                    ))
                    .strong(),
                );
            } else {
                ui.label("Click a binding to record a key combination. Esc cancels recording.");
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.ui.settings_ui.shortcut_query)
                    .desired_width(f32::INFINITY)
                    .hint_text("Filter shortcuts by command or assigned key…"),
            );
            if let Some((action, binding, other)) = self.ui.settings_ui.conflict {
                ui.label(
                    RichText::new(format!(
                        "{binding} is assigned to “{}”. Move it to “{}”?",
                        other.label(),
                        action.label()
                    ))
                    .strong(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Move binding").clicked() {
                        self.settings.keymap.set(action, Some(binding));
                        self.ui.settings_ui.conflict = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.ui.settings_ui.conflict = None;
                    }
                });
            }
            if ui.button("Reset all shortcuts…").clicked() {
                self.ui.settings_ui.confirm_reset = true;
                self.ui.capturing = None;
            }
            if self.ui.settings_ui.confirm_reset {
                ui.label("Reset every shortcut to its default binding?");
                ui.horizontal(|ui| {
                    if ui.button("Reset all").clicked() {
                        self.settings.keymap = Keymap::default();
                        self.ui.settings_ui.confirm_reset = false;
                        self.ui.settings_ui.conflict = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.ui.settings_ui.confirm_reset = false;
                    }
                });
            }
        });
        let filter = self.ui.settings_ui.shortcut_query.to_lowercase();
        let mut count = 0;
        for name in [
            "General", "Playback", "Preview", "Marking", "Export", "Timeline",
        ] {
            let actions: Vec<_> = Action::ALL
                .into_iter()
                .filter(|a| {
                    a.group() == name
                        && format!("{} {}", a.label(), self.settings.keymap.text(*a))
                            .to_lowercase()
                            .contains(&filter)
                })
                .collect();
            if actions.is_empty() {
                continue;
            }
            group(ui, name);
            for action in actions {
                ui.push_id(action, |ui| {
                    // Use a stacked layout in narrow windows; wide windows share aligned columns.
                    let narrow = ui.available_width() < 560.0;
                    let label_width = (ui.available_width() - 270.0).max(140.0);
                    let binding = self.settings.keymap.get(action);
                    let capturing = self.ui.capturing == Some(action);
                    let text = if capturing {
                        "Waiting for input…".into()
                    } else {
                        binding.map_or("Unbound".into(), |b| b.to_string())
                    };
                    if narrow {
                        ui.label(action.label());
                    }
                    ui.horizontal(|ui| {
                        if !narrow {
                            ui.add_sized(
                                [label_width, 24.0],
                                egui::Label::new(action.label())
                                    .wrap()
                                    .halign(egui::Align::LEFT),
                            );
                        }
                        if ui
                            .add_sized(
                                [140.0, 26.0],
                                egui::Button::selectable(
                                    capturing,
                                    RichText::new(text).monospace(),
                                ),
                            )
                            .clicked()
                        {
                            self.ui.capturing = if capturing { None } else { Some(action) };
                            self.ui.settings_ui.conflict = None;
                        }
                        if ui
                            .add_enabled(binding.is_some(), egui::Button::new("Clear"))
                            .clicked()
                        {
                            self.propose_binding(action, None);
                        }
                        if ui
                            .add_enabled(
                                binding != default_binding(action),
                                egui::Button::new("Reset"),
                            )
                            .clicked()
                        {
                            self.propose_binding(action, default_binding(action));
                        }
                    });
                    ui.add_space(4.0);
                });
                count += 1;
            }
        }
        if count == 0 {
            ui.label("No matching shortcuts.");
        }
    }
}

pub(super) fn shell_executable(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        format!("\"{path}\"")
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}
