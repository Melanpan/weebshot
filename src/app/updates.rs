//! Update window and the daily background check.

use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText};
use egui_commonmark::CommonMarkViewer;

use super::App;
use crate::fonts::icon;
use crate::update::{self, State};

const DAY: u64 = 24 * 60 * 60;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl App {
    /// Check once a day at startup (unless turned off, or during the docs screenshot run).
    pub(super) fn maybe_check_for_updates(&mut self, ctx: &egui::Context) {
        if self.script.is_some() || !self.settings.check_updates {
            return;
        }
        if now().saturating_sub(self.settings.last_update_check) >= DAY {
            self.settings.last_update_check = now();
            self.updater.check(ctx);
        }
    }

    pub(super) fn poll_updater(&mut self, ctx: &egui::Context) {
        self.updater.poll();
        if self.updater.state == State::ReadyToRestart {
            // The helper script / new AppImage takes it from here.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub(super) fn update_window(&mut self, ctx: &egui::Context) {
        if !self.ui.show_update {
            return;
        }
        let open = super::native_window(
            ctx,
            "update",
            "Weebshot — Updates",
            [560.0, 440.0],
            [420.0, 260.0],
            |ui| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(self.palette.bg1)
                            .inner_margin(egui::Margin::same(16)),
                    )
                    .show(ui, |ui| self.update_contents(ui));
            },
        );
        if !open {
            self.ui.show_update = false;
        }
    }

    fn update_contents(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        ui.label(format!("You have Weebshot {}.", update::current_version()));
        ui.add_space(8.0);
        match self.updater.state.clone() {
            State::Idle => {
                if ui.button("Check for updates").clicked() {
                    self.updater.check(ui.ctx());
                }
            }
            State::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking GitHub…");
                });
            }
            State::UpToDate => {
                ui.colored_label(p.ok, format!("{} You're up to date.", icon::CHECK));
            }
            State::Failed(e) => {
                ui.colored_label(p.error, format!("{} {e}", icon::WARNING));
                ui.horizontal(|ui| {
                    if ui.button("Try again").clicked() {
                        self.updater.check(ui.ctx());
                    }
                    if ui.button("Open releases page").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                            "https://github.com/{}/releases",
                            update::repo()
                        )));
                    }
                });
            }
            State::Installing => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Downloading and installing… Weebshot restarts when it's done.");
                });
            }
            State::ReadyToRestart => {
                ui.label("Restarting…");
            }
            State::Available(release) => {
                ui.heading(format!("Weebshot {} is available", release.version));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_install = update::can_self_update() && release.download.is_some();
                    if can_install
                        && ui
                            .button(
                                RichText::new(format!("{} Update and restart", icon::DOWNLOAD))
                                    .strong(),
                            )
                            .clicked()
                    {
                        self.updater.install(release.clone(), ui.ctx());
                    }
                    if ui.button("Open release page").clicked() {
                        ui.ctx()
                            .open_url(egui::OpenUrl::new_tab(release.page.clone()));
                    }
                    if ui.button("Skip this version").clicked() {
                        self.settings.skipped_version = Some(release.version.clone());
                        self.ui.show_update = false;
                    }
                });
                if !update::can_self_update() {
                    ui.weak("This build can't update itself — download the new version from the release page.");
                } else if self.jobs_active() > 0 {
                    ui.colored_label(
                        p.warn,
                        "Exports are still running; updating restarts Weebshot and stops them.",
                    );
                }
                ui.add_space(8.0);
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let notes = if release.notes.trim().is_empty() {
                            "*No release notes.*".to_string()
                        } else {
                            release.notes.clone()
                        };
                        CommonMarkViewer::new().show(ui, &mut self.ui.docs_cache, &notes);
                    });
            }
        }
    }
}
