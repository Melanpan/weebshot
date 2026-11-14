//! Small settings-only layout and search helpers. Each canonical row uses a Setting id.
use super::OptionsPage;
use eframe::egui::{self, RichText};

impl OptionsPage {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Playback => "Playback",
            Self::Export => "Export",
            Self::Appearance => "Appearance",
            Self::Shortcuts => "Shortcuts",
            Self::Advanced => "Advanced",
        }
    }
}

pub(super) struct Metadata {
    pub category: OptionsPage,
    pub section: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub keywords: &'static str,
}

macro_rules! settings {
    ($($id:ident, $page:ident, $section:literal, $label:literal, $description:literal, $keywords:literal;)+) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(super) enum Setting { $($id,)+ }
        impl Setting {
            pub const ALL: &'static [Self] = &[$(Self::$id,)+];
            pub fn meta(self) -> Metadata {
                match self { $(Self::$id => Metadata { category: OptionsPage::$page, section: $section,
                    label: $label, description: $description, keywords: $keywords },)+ }
            }
        }
    }
}
settings! {
    Output, General, "Output", "Default output directory", "Save next to the video or choose a folder.", "path folder same as video";
    ShotName, General, "Output", "Screenshot filename template", "Name screenshots using filename tokens.", "naming output tokens name time frame";
    ClipName, General, "Output", "Clip filename template", "Name clips using filename tokens.", "naming output tokens start end";
    Sounds, General, "Sounds", "Enable sound effects", "Play sounds for screenshots and completed or failed exports.", "audio notifications";
    SoundVolume, General, "Sounds", "Sound volume and previews", "Adjust volume and preview each sound.", "test audio notification";
    Updates, General, "Updates", "Automatic update checking", "Check GitHub once a day or check manually.", "version new release check now";
    Audio, Playback, "Track selection", "Preferred audio languages", "Comma-separated codes in order of preference; applies to newly opened files.", "track selection language priority";
    Subtitles, Playback, "Track selection", "Preferred subtitle languages", "Comma-separated codes in order of preference; applies to newly opened files.", "track selection language priority subs";
    ShortStep, Playback, "Stepping", "Short step", "Distance and unit for the short step shortcuts.", "frames seconds backward forward";
    MediumStep, Playback, "Stepping", "Medium step", "Distance and unit for the medium step shortcuts.", "frames seconds backward forward";
    LongStep, Playback, "Stepping", "Long step", "Distance and unit for the long step shortcuts.", "frames seconds backward forward";
    Decode, Playback, "Decoding", "Hardware decoding", "Use GPU acceleration to decode video. Disable if frames glitch.", "performance graphics";
    Ffmpeg, Export, "External tools", "FFmpeg executable", "Detected version, path and executable selection.", "binary tools browse auto-detect";
    Gifsicle, Export, "External tools", "Gifsicle executable", "Optional tool for smaller GIFs.", "binary tools browse auto-detect path";
    Stitch, Export, "Auto Stitch", "Stitch method", "Combine clean source frames into a panorama. Force a result builds a rough stitch when alignment fails.", "stitching frames automatic translation layers motion divider foreground background force low confidence unverified rough";
    StitchAdvanced, Export, "Auto Stitch", "Advanced stitch options", "Frame spacing, analysis frames per second, search window and pan pauses.", "sampling fps travel panorama stitching seconds";
    H264, Export, "Video encoding", "H.264 preset", "Slower presets give smaller MP4/MKV files at the same quality.", "x264 encoder compression speed";
    Vp9, Export, "Video encoding", "VP9 speed", "WebM encoding: 0 is best quality, 5 is fastest.", "encoder compression performance";
    Hardware, Export, "Hardware encoding", "Hardware encoding", "Detect GPU encoders and select one for MP4/MKV.", "acceleration NVENC VAAPI AMF QSV encoder graphics availability";
    Theme, Appearance, "Theme", "Colour theme", "Choose a theme for every window.", "color gruvbox dark light nord catppuccin dracula studio";
    Scale, Appearance, "Interface", "Interface size", "Adjust the interface scale.", "zoom DPI scaling percentage";
    Bindings, Shortcuts, "Bindings", "Keyboard shortcuts", "Search command names or assigned keys; record, clear or reset bindings.", "keymap hotkeys conflict reset";
    Mcp, Advanced, "Automation / MCP", "Enable MCP integration", "Optional local automation for media processing and the GUI.", "agent tools AI security trust disable";
    GuiCommand, Advanced, "Automation / MCP", "GUI attachment command", "Connect to the running GUI using its local socket.", "MCP automation agent AI launch";
    HeadlessCommand, Advanced, "Automation / MCP", "Headless launch command", "Run media processing without a GUI.", "MCP automation agent AI launch";
    Socket, Advanced, "Automation / MCP", "Local socket path", "Local connection used by trusted MCP clients.", "MCP automation IPC agent";
    McpConfig, Advanced, "Automation / MCP", "MCP client configuration", "Copy configuration or open the documentation.", "agent automation tools AI json";
}
impl Setting {
    pub fn matches(self, query: &str) -> bool {
        let m = self.meta();
        let haystack = format!(
            "{} {} {} {} {}",
            m.category.label(),
            m.section,
            m.label,
            m.description,
            m.keywords
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

#[derive(Default)]
pub(super) struct State {
    pub query: String,
    pub shortcut_query: String,
    pub previous_page: Option<OptionsPage>,
    pub conflict: Option<(
        crate::keymap::Action,
        crate::keymap::Binding,
        crate::keymap::Action,
    )>,
    pub confirm_reset: bool,
}
impl State {
    pub fn propose_binding(
        &mut self,
        keymap: &mut crate::keymap::Keymap,
        action: crate::keymap::Action,
        binding: Option<crate::keymap::Binding>,
    ) {
        self.conflict = None;
        if let Some(binding) = binding
            && let Some(other) = crate::keymap::Action::ALL
                .into_iter()
                .find(|other| *other != action && keymap.get(*other) == Some(binding))
        {
            self.conflict = Some((action, binding, other));
        } else {
            keymap.set(action, binding);
        }
    }

    pub fn restore_page(&mut self, page: &mut OptionsPage) {
        if let Some(previous) = self.previous_page.take() {
            *page = previous;
        }
    }
    pub fn navigate(&mut self, setting: Setting, page: &mut OptionsPage, ctx: &egui::Context) {
        self.query.clear();
        self.previous_page = None;
        *page = setting.meta().category;
        let until = ctx.input(|i| i.time) + 1.5;
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::unique("settings_target"),
                Target {
                    setting,
                    until,
                    scroll: true,
                },
            )
        });
    }
}
#[derive(Clone, Copy)]
struct Target {
    setting: Setting,
    until: f64,
    scroll: bool,
}
pub(super) fn in_settings(ui: &egui::Ui) -> bool {
    ui.stack()
        .iter()
        .any(|node| node.tags().contains("weebshot_settings"))
}
pub(super) fn targeted(ui: &egui::Ui, setting: Setting) -> bool {
    ui.data(|d| d.get_temp::<Target>(egui::Id::unique("settings_target")))
        .is_some_and(|t| t.setting == setting && ui.input(|i| i.time) < t.until)
}
pub(super) fn group(ui: &mut egui::Ui, title: &str) {
    if ui.cursor().top() > ui.max_rect().top() + 1.0 {
        ui.add_space(12.0);
    }
    ui.label(RichText::new(title).size(16.0).strong());
    ui.add_space(4.0);
}

/// Stacked canonical row. Stable id also scopes text editing and keyboard focus.
pub(super) fn row<R>(
    ui: &mut egui::Ui,
    setting: Setting,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    if !in_settings(ui) {
        return add(ui);
    }
    let result = ui
        .push_id(setting, |ui| {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                add(ui)
            })
        })
        .inner;
    let id = egui::Id::unique("settings_target");
    if let Some(mut target) = ui.data(|d| d.get_temp::<Target>(id))
        && target.setting == setting
    {
        if target.scroll {
            ui.scroll_to_rect(result.response.rect, Some(egui::Align::Center));
            target.scroll = false;
            ui.data_mut(|d| d.insert_temp(id, target));
        }
        if ui.input(|i| i.time) < target.until {
            ui.painter().rect_stroke(
                result.response.rect.expand(3.0),
                3.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    ui.add_space(4.0);
    result.inner
}
pub(super) fn text(ui: &mut egui::Ui, setting: Setting, value: &mut String) -> egui::Response {
    row(ui, setting, |ui| {
        let m = setting.meta();
        ui.label(m.label);
        let r = ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
        if targeted(ui, setting)
            && ui
                .data(|d| d.get_temp::<Target>(egui::Id::unique("settings_target")))
                .is_some_and(|t| t.scroll)
        {
            r.request_focus();
        }
        r
    })
}
/// Read-only selectable code; wraps long paths/commands within the available width.
pub(super) fn code(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(value.to_owned());
        }
    });
    let mut value = value;
    ui.add(
        egui::TextEdit::multiline(&mut value)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .desired_rows(1),
    );
}

pub(super) fn concise_version(name: &str, details: &str) -> String {
    let words: Vec<_> = details.split_whitespace().collect();
    let version = words
        .iter()
        .position(|w| w.eq_ignore_ascii_case("version"))
        .and_then(|i| words.get(i + 1).copied())
        .or_else(|| {
            words
                .iter()
                .copied()
                .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))
        })
        .unwrap_or("detected");
    format!("{name} {version}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_and_case_and_multiple_words() {
        for (query, setting) in [
            ("GPU", Setting::Decode),
            ("nvenc", Setting::Hardware),
            ("panorama", Setting::Stitch),
            ("AI", Setting::Mcp),
            ("naming tokens", Setting::ClipName),
            ("dpi", Setting::Scale),
            ("binary", Setting::Ffmpeg),
        ] {
            assert!(setting.matches(query), "{query}");
        }
        assert!(!Setting::Decode.matches("gpu bananas"));
        assert!(
            Setting::ALL
                .iter()
                .all(|s| s.matches(s.meta().category.label()))
        );
        assert!(!Setting::ALL.iter().any(|s| s.matches("no-such-setting")));
    }
    #[test]
    fn search_navigation_and_clear_restore() {
        let ctx = egui::Context::default();
        let mut state = State {
            previous_page: Some(OptionsPage::Playback),
            query: "gpu".into(),
            ..Default::default()
        };
        let mut page = OptionsPage::General;
        state.restore_page(&mut page);
        assert!(page == OptionsPage::Playback);
        for setting in Setting::ALL {
            state.navigate(*setting, &mut page, &ctx);
            assert!(page == setting.meta().category);
            assert!(state.query.is_empty());
            assert!(state.previous_page.is_none());
            assert_eq!(
                ctx.data(|d| d.get_temp::<Target>(egui::Id::unique("settings_target")))
                    .unwrap()
                    .setting,
                *setting
            );
        }
    }
    #[test]
    fn conflicting_bindings_wait_for_confirmation_including_reset() {
        use crate::keymap::{Action, Binding, Keymap, default_binding};
        let mut keymap = Keymap::default();
        let mut state = State::default();
        let taken = keymap.get(Action::SetIn).unwrap();
        state.propose_binding(&mut keymap, Action::Screenshot, Some(taken));
        assert_eq!(keymap.get(Action::SetIn), Some(taken));
        assert_eq!(
            keymap.get(Action::Screenshot),
            default_binding(Action::Screenshot)
        );
        assert_eq!(
            state.conflict,
            Some((Action::Screenshot, taken, Action::SetIn))
        );
        // Cancelling never touches either command. Individual reset uses this same path.
        state.conflict = None;
        keymap.set(Action::Screenshot, Some(Binding::key(egui::Key::F8)));
        let original = default_binding(Action::Screenshot);
        keymap.set(Action::SetIn, original);
        state.propose_binding(&mut keymap, Action::Screenshot, original);
        assert_eq!(keymap.get(Action::SetIn), original);
        let (action, binding, other) = state.conflict.take().unwrap();
        keymap.set(action, Some(binding));
        assert_eq!(keymap.get(other), None);
        assert_eq!(keymap.get(action), Some(binding));
    }

    #[test]
    fn canonical_text_navigation_focuses_and_scrolls_without_touching_sidebar() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut page = OptionsPage::General;
        state.navigate(Setting::Audio, &mut page, &ctx);
        let mut value = "jpn".to_owned();
        let mut focus_id = None;
        for frame in 0..12 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(440.0, 300.0),
                    )),
                    time: Some(frame as f64 * 0.05),
                    ..Default::default()
                },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        // Shared controls outside Settings must not consume navigation.
                        row(ui, Setting::Audio, |ui| {
                            ui.label("Main sidebar");
                        });
                        if frame == 0 {
                            assert!(
                                ctx.data(
                                    |d| d.get_temp::<Target>(egui::Id::unique("settings_target"))
                                )
                                .unwrap()
                                .scroll
                            );
                        }
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.scope_builder(
                                egui::UiBuilder::new().ui_stack_info(
                                    egui::UiStackInfo::default().with_tag("weebshot_settings"),
                                ),
                                |ui| {
                                    ui.add_space(800.0);
                                    let response = text(ui, Setting::Audio, &mut value);
                                    focus_id = Some(response.id);
                                    if frame == 11 {
                                        assert!(
                                            ui.clip_rect().intersects(response.rect),
                                            "clip {:?}, row {:?}",
                                            ui.clip_rect(),
                                            response.rect
                                        );
                                    }
                                },
                            );
                        });
                    });
                },
            );
            output.textures_delta.clear();
        }
        assert_eq!(ctx.memory(|m| m.focused()), focus_id);
        assert_eq!(value, "jpn");
        assert!(
            !ctx.data(|d| d.get_temp::<Target>(egui::Id::unique("settings_target")))
                .unwrap()
                .scroll
        );
    }

    #[test]
    fn long_commands_and_paths_wrap_in_every_theme() {
        for theme in crate::theme::Theme::ALL {
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            let value = format!("/{} --headless --mcp", "long-directory/".repeat(40));
            for _ in 0..2 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(440.0, 400.0),
                        )),
                        ..Default::default()
                    },
                    |root| {
                        egui::CentralPanel::default().show(root, |ui| {
                            let width = ui.available_width();
                            code(ui, "GUI attachment command", &value);
                            assert!(
                                ui.min_rect().width() <= width + 1.0,
                                "{}: command widened its container",
                                theme.label()
                            );
                        });
                    },
                );
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn tool_banner_is_concise() {
        assert_eq!(
            concise_version("FFmpeg", "ffmpeg version 9.0.2 Copyright (c) developers"),
            "FFmpeg 9.0.2"
        );
        assert_eq!(
            concise_version("Gifsicle", "LCDF Gifsicle 1.96"),
            "Gifsicle 1.96"
        );
    }
}
