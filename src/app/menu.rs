use eframe::egui;

use super::{App, OptionsPage, SideTab};
use crate::fonts::icon;
use crate::keymap::Action;
use crate::theme::Theme;
use crate::tracks::TrackKind;

impl App {
    /// Menu entry with its shortcut on the right; queues the action when clicked.
    fn menu_item(&mut self, ui: &mut egui::Ui, action: Action, label: &str, enabled: bool) {
        let shortcut = self.settings.keymap.text(action);
        let button = egui::Button::new(label).shortcut_text(shortcut);
        if ui.add_enabled(enabled, button).clicked() {
            self.pending.push(action);
        }
    }

    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let loaded = self.player.file.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                self.menu_item(ui, Action::Open, "Open…", true);
                ui.menu_button("Open recent", |ui| {
                    if self.settings.recent.is_empty() {
                        ui.weak("Nothing yet");
                    }
                    let mut open = None;
                    for path in &self.settings.recent {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        if ui
                            .button(name.as_ref())
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            open = Some(path.clone());
                        }
                    }
                    if !self.settings.recent.is_empty() {
                        ui.separator();
                        if ui.button("Clear list").clicked() {
                            self.settings.recent.clear();
                        }
                    }
                    if let Some(p) = open {
                        self.open(&p);
                    }
                });
                self.menu_item(ui, Action::Close, "Close", loaded);
                ui.separator();
                self.menu_item(
                    ui,
                    Action::Options,
                    &format!("{} Options…", icon::GEAR),
                    true,
                );
                ui.separator();
                self.menu_item(ui, Action::Quit, "Quit", true);
            });

            ui.menu_button("Mark", |ui| {
                self.menu_item(ui, Action::SetIn, "Set In", loaded);
                self.menu_item(ui, Action::SetOut, "Set Out", loaded);
                self.menu_item(ui, Action::MarkSubtitleLine, "Mark subtitle line", loaded);
                self.menu_item(ui, Action::ClearInOut, "Clear In/Out", self.has_range());
                ui.separator();
                self.menu_item(ui, Action::GoToIn, "Go to In", self.in_point.is_some());
                self.menu_item(ui, Action::GoToOut, "Go to Out", self.out_point.is_some());
                ui.separator();
                let label = if self.looping {
                    &format!("{} Loop In→Out", crate::fonts::icon::CHECK)
                } else {
                    "Loop In→Out"
                };
                self.menu_item(ui, Action::ToggleLoop, label, loaded);
            });

            ui.menu_button("Playback", |ui| {
                let play = if self.player.paused { "Play" } else { "Pause" };
                self.menu_item(ui, Action::PlayPause, play, loaded);
                ui.separator();
                self.menu_item(ui, Action::FrameBack, "Previous frame", loaded);
                self.menu_item(ui, Action::FrameForward, "Next frame", loaded);
                self.menu_item(ui, Action::SeekBackSmall, "Back (small step)", loaded);
                self.menu_item(ui, Action::SeekForwardSmall, "Forward (small step)", loaded);
                self.menu_item(ui, Action::SeekBackBig, "Back (big step)", loaded);
                self.menu_item(ui, Action::SeekForwardBig, "Forward (big step)", loaded);
                ui.separator();
                self.menu_item(ui, Action::PrevSubtitle, "Previous subtitle line", loaded);
                self.menu_item(ui, Action::NextSubtitle, "Next subtitle line", loaded);
                ui.separator();
                for (kind, label) in [
                    (TrackKind::Audio, "Audio track"),
                    (TrackKind::Sub, "Subtitle track"),
                ] {
                    ui.add_enabled_ui(loaded, |ui| {
                        ui.menu_button(label, |ui| self.track_list(ui, kind));
                    });
                }
            });

            ui.menu_button("Export", |ui| {
                self.menu_item(ui, Action::Screenshot, "Screenshot", loaded);
                self.menu_item(ui, Action::ExportClip, "Export clip", loaded);
                ui.separator();
                if ui.button("Show queue").clicked() {
                    self.settings.show_inspector = true;
                    self.ui.side_tab = SideTab::Queue;
                }
                if ui.button("Export settings…").clicked() {
                    self.ui.show_options = true;
                    self.ui.options_page = OptionsPage::Export;
                }
            });

            ui.menu_button("View", |ui| {
                ui.menu_button("Theme", |ui| {
                    for t in Theme::ALL {
                        if ui.radio(self.settings.theme == t, t.label()).clicked() {
                            self.set_theme(ui.ctx(), t);
                        }
                    }
                });
                ui.checkbox(
                    &mut self.settings.show_inspector,
                    "Side panel (Inspector, Queue, Logs)",
                );
                ui.checkbox(
                    &mut self.settings.show_sub_lane,
                    "Subtitle lane in timeline",
                );
                ui.separator();
                self.menu_item(ui, Action::ZoomIn, "Zoom timeline in", loaded);
                self.menu_item(ui, Action::ZoomOut, "Zoom timeline out", loaded);
                self.menu_item(ui, Action::ZoomFit, "Fit timeline", loaded);
            });

            ui.menu_button("Help", |ui| {
                self.menu_item(
                    ui,
                    Action::Docs,
                    &format!("{} Documentation", icon::QUESTION),
                    true,
                );
                if ui.button("Keyboard shortcuts").clicked() {
                    self.ui.show_docs = true;
                    self.ui.docs_page = super::docs::page_index(super::docs::SHORTCUTS_TITLE);
                }
                if ui.button("Show logs").clicked() {
                    self.settings.show_inspector = true;
                    self.ui.side_tab = SideTab::Logs;
                }
                ui.separator();
                if ui.button("Check for updates…").clicked() {
                    self.updater.check(ui.ctx());
                    self.ui.show_update = true;
                }
                if ui.button("About weebshot").clicked() {
                    self.ui.show_about = true;
                }
            });
        });
    }

    /// Radio list of tracks of one kind (used by menus and the inspector).
    pub(super) fn track_list(&mut self, ui: &mut egui::Ui, kind: TrackKind) {
        let selected = match kind {
            TrackKind::Audio => self.player.aid,
            _ => self.player.sid,
        };
        let mut choice = selected;
        ui.radio_value(&mut choice, None, "None");
        for t in self.player.tracks.iter().filter(|t| t.kind == kind) {
            ui.radio_value(&mut choice, Some(t.id), t.label());
        }
        if choice != selected {
            self.player.select_track(kind, choice);
        }
    }

    pub(super) fn set_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.settings.theme = theme;
        self.palette = theme.palette();
        theme.apply(ctx);
    }
}
