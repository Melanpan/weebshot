use std::sync::atomic::Ordering;

use eframe::egui::{self, RichText};

use super::{App, JobState, dot, widgets};
use crate::fonts::icon;
use crate::log::{self, Level};
use crate::util::{fmt_time, human_size, reveal};

impl App {
    pub(super) fn queue_tab(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        if self.jobs.is_empty() {
            ui.add_space(8.0);
            ui.weak("No exports yet. Mark In/Out on the timeline and press Export clip.");
            return;
        }
        ui.horizontal(|ui| {
            if ui.small_button("Clear finished").clicked() {
                self.jobs
                    .retain(|j| matches!(j.state, JobState::Queued | JobState::Running { .. }));
            }
            if ui.small_button("Cancel all").clicked() {
                for j in &self.jobs {
                    j.cancel.store(true, Ordering::Relaxed);
                }
            }
        });
        ui.add_space(4.0);

        let mut retry = None;
        let mut remove = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for job in self.jobs.iter().rev() {
                    egui::Frame::new()
                        .fill(p.bg2.gamma_multiply(0.6))
                        .corner_radius(4)
                        .inner_margin(6)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let color = match job.state {
                                JobState::Queued => p.fg_dim,
                                JobState::Running { .. } => p.accent,
                                JobState::Done => p.ok,
                                JobState::Failed(_) => p.error,
                            };
                            ui.horizontal(|ui| {
                                match &job.state {
                                    JobState::Running { step, .. } => {
                                        widgets::progress_ring(
                                            ui,
                                            step.map(|_| job.progress),
                                            16.0,
                                            p.accent,
                                            p.bg3,
                                        );
                                    }
                                    _ => dot(ui, color),
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        match job.state {
                                            JobState::Queued | JobState::Running { .. } => {
                                                if ui
                                                    .small_button(icon::CLOSE)
                                                    .on_hover_text("Cancel")
                                                    .clicked()
                                                {
                                                    job.cancel.store(true, Ordering::Relaxed);
                                                }
                                            }
                                            JobState::Done => {
                                                if ui
                                                    .small_button(icon::TRASH)
                                                    .on_hover_text("Remove from list")
                                                    .clicked()
                                                {
                                                    remove = Some(job.id);
                                                }
                                                if ui
                                                    .small_button(icon::FOLDER_OPEN)
                                                    .on_hover_text("Show in folder")
                                                    .clicked()
                                                    && let Some(dir) = job.job.output.parent()
                                                {
                                                    reveal(dir, Some(&job.job.output));
                                                }
                                            }
                                            JobState::Failed(_) => {
                                                if ui
                                                    .small_button(icon::TRASH)
                                                    .on_hover_text("Remove from list")
                                                    .clicked()
                                                {
                                                    remove = Some(job.id);
                                                }
                                                if ui
                                                    .small_button(icon::RETRY)
                                                    .on_hover_text("Retry")
                                                    .clicked()
                                                {
                                                    retry = Some(job.id);
                                                }
                                            }
                                        }
                                        ui.add(
                                            egui::Label::new(RichText::new(&job.name).strong())
                                                .truncate(),
                                        )
                                        .on_hover_text(job.job.output.display().to_string());
                                    },
                                );
                            });
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} → {}",
                                    job.job.format.short(),
                                    fmt_time(job.job.start),
                                    fmt_time(job.job.end)
                                ))
                                .weak()
                                .small(),
                            );
                            match &job.state {
                                JobState::Queued => {
                                    ui.weak("Waiting…");
                                }
                                JobState::Running { stage, step } => {
                                    let bar =
                                        egui::ProgressBar::new(job.progress).text(match step {
                                            Some(_) => {
                                                format!("{stage} · {:.0}%", job.progress * 100.0)
                                            }
                                            None => format!("{stage}…"),
                                        });
                                    ui.add(if step.is_none() {
                                        bar.animate(true)
                                    } else {
                                        bar
                                    });
                                }
                                JobState::Done => {
                                    let size = std::fs::metadata(&job.job.output)
                                        .map(|m| m.len())
                                        .unwrap_or(0);
                                    let took = job
                                        .elapsed
                                        .map(|d| format!(" in {:.1}s", d.as_secs_f32()))
                                        .unwrap_or_default();
                                    ui.colored_label(
                                        p.ok,
                                        format!("{} {}{took}", icon::CHECK, human_size(size)),
                                    );
                                }
                                JobState::Failed(e) => {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("{} {e}", icon::X_CIRCLE))
                                                .color(p.error),
                                        )
                                        .wrap(),
                                    )
                                    .on_hover_text("Full ffmpeg output is in the Logs tab");
                                }
                            }
                        });
                    ui.add_space(4.0);
                }
            });

        if let Some(id) = retry
            && let Some(pos) = self.jobs.iter().position(|j| j.id == id)
        {
            let job = self.jobs.remove(pos).job;
            self.submit(job);
        }
        if let Some(id) = remove {
            self.jobs.retain(|j| j.id != id);
        }
    }

    pub(super) fn logs_tab(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let view = &mut self.ui.log_view;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("log_level")
                .selected_text(view.min_level.map_or("all levels", |l| l.label()))
                .width(90.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut view.min_level, None, "all levels");
                    for l in Level::ALL {
                        ui.selectable_value(
                            &mut view.min_level,
                            Some(l),
                            format!("{}+", l.label()),
                        );
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut view.search)
                    .hint_text("filter…")
                    .desired_width(180.0),
            );
            ui.checkbox(&mut view.follow, "Follow");
            if ui.small_button("Copy").clicked() {
                let text = log::with_lines(|lines| {
                    lines
                        .iter()
                        .map(|l| {
                            format!(
                                "{:>9.3} {:5} {:8} {}",
                                l.time,
                                l.level.label(),
                                l.source,
                                l.text
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                ui.ctx().copy_text(text);
            }
            if ui.small_button("Clear").clicked() {
                log::clear();
            }
        });

        let search = view.search.to_lowercase();
        let min = view.min_level.unwrap_or(Level::Debug);
        let lines: Vec<log::Line> = log::with_lines(|lines| {
            lines
                .iter()
                .filter(|l| l.level >= min)
                .filter(|l| {
                    search.is_empty()
                        || l.text.to_lowercase().contains(&search)
                        || l.source.contains(&search)
                })
                .cloned()
                .collect()
        });

        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::both()
            .auto_shrink(false)
            .stick_to_bottom(view.follow)
            .show_rows(ui, row_h, lines.len(), |ui, range| {
                for l in &lines[range] {
                    let color = match l.level {
                        Level::Debug => p.fg_dim,
                        Level::Info => p.fg,
                        Level::Warn => p.warn,
                        Level::Error => p.error,
                    };
                    ui.label(
                        RichText::new(format!(
                            "{:>9.3}  {:5}  {:8}  {}",
                            l.time,
                            l.level.label(),
                            l.source,
                            l.text
                        ))
                        .monospace()
                        .color(color),
                    );
                }
            });
    }
}
