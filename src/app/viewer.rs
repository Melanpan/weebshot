use eframe::egui::{self, Align2, FontId, RichText, Sense, vec2};

use super::timeline::MarkerPreview;
use super::{App, widgets};
use crate::fonts::{filled, icon};
use crate::keymap::Action;
use crate::tracks::TrackKind;
use crate::util::{fmt_time, fmt_timecode, frame_at, parse_position};

const TRANSPORT_HEIGHT: f32 = 44.0;

/// How the video is framed in the preview.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ViewZoom {
    /// Whole picture, letterboxed.
    #[default]
    Fit,
    /// Fixed scale: 1.0 = one video pixel per screen pixel.
    Scale(f32),
    /// Crop to fill the viewer.
    Fill,
}

const ASPECTS: &[(Option<&str>, &str)] = &[
    (None, "Auto"),
    (Some("16:9"), "16:9"),
    (Some("4:3"), "4:3"),
    (Some("2.39:1"), "2.39:1 (scope)"),
    (Some("1:1"), "1:1"),
];

#[derive(Default)]
pub struct ViewState {
    pub zoom: ViewZoom,
    pub aspect: Option<&'static str>,
    /// Pan in fractions of the displayed video.
    pan: (f64, f64),
    /// Last values sent to mpv, rounded, so properties are only set on change.
    applied: Option<(i64, i64, i64, i64, Option<&'static str>)>,
}

fn parse_aspect(a: &str) -> Option<f64> {
    let (w, h) = a.split_once(':')?;
    Some(w.parse::<f64>().ok()? / h.parse::<f64>().ok()?)
}

impl App {
    /// Displayed video size in physical pixels for the current zoom, and mpv's video-zoom.
    fn view_geometry(&self, rect_px: egui::Vec2) -> Option<(egui::Vec2, f64)> {
        let (vw, vh) = (
            self.player.video_width? as f64,
            self.player.video_height? as f64,
        );
        let vw = self
            .view
            .aspect
            .and_then(parse_aspect)
            .map_or(vw, |a| vh * a);
        let (rw, rh) = (rect_px.x as f64, rect_px.y as f64);
        let fit = (rw / vw).min(rh / vh);
        let scale = match self.view.zoom {
            ViewZoom::Fit => fit,
            ViewZoom::Scale(s) => s as f64,
            ViewZoom::Fill => (rw / vw).max(rh / vh),
        };
        let zoom = match self.view.zoom {
            ViewZoom::Scale(s) => (s as f64 / fit).log2(),
            _ => 0.0,
        };
        Some((egui::vec2((vw * scale) as f32, (vh * scale) as f32), zoom))
    }

    fn apply_view(&mut self, rect_px: egui::Vec2) {
        let zoom = self.view_geometry(rect_px).map_or(0.0, |(_, z)| z);
        let panscan = if self.view.zoom == ViewZoom::Fill {
            1.0
        } else {
            0.0
        };
        let key = |v: f64| (v * 1000.0).round() as i64;
        let wanted = (
            key(zoom),
            key(panscan),
            key(self.view.pan.0),
            key(self.view.pan.1),
            self.view.aspect,
        );
        if self.view.applied != Some(wanted) {
            self.view.applied = Some(wanted);
            self.player.set_view(
                zoom,
                panscan,
                self.view.pan,
                self.view.aspect.unwrap_or("no"),
            );
        }
    }

    /// Ctrl+wheel over the preview: zoom by `factor`, keeping the point under the
    /// pointer where it is.
    fn zoom_view_at(&mut self, factor: f32, pointer: egui::Pos2, rect: egui::Rect, ppp: f32) {
        let rect_px = rect.size() * ppp;
        let Some((shown, _)) = self.view_geometry(rect_px) else {
            return;
        };
        let current = self.view.zoom;
        self.view.zoom = ViewZoom::Fit;
        let fit_shown = self.view_geometry(rect_px).map_or(shown, |(s, _)| s);
        self.view.zoom = current;
        let scale = self.view_scale(shown);
        let fit = self.view_scale(fit_shown);
        let new = (scale * factor).clamp(fit * 0.5, 8.0);
        if (new / fit - 1.0).abs() < 0.04 {
            self.set_zoom(ViewZoom::Fit);
            return;
        }
        // Pointer relative to the viewer center, in pixels; the video's center sits at
        // pan × shown size from there.
        let p = (pointer - rect.center()) * ppp;
        let u = egui::vec2(
            (p.x - self.view.pan.0 as f32 * shown.x) / shown.x,
            (p.y - self.view.pan.1 as f32 * shown.y) / shown.y,
        );
        let shown_new = shown * (new / scale);
        let clamp = |v: f32| (v as f64).clamp(-0.5, 0.5);
        self.view.pan = (
            clamp(p.x / shown_new.x - u.x),
            clamp(p.y / shown_new.y - u.y),
        );
        self.view.zoom = ViewZoom::Scale(new);
    }

    /// Screen pixels per video pixel for a displayed size.
    fn view_scale(&self, shown: egui::Vec2) -> f32 {
        let vw = self.player.video_width.unwrap_or(1) as f32;
        let vh = self.player.video_height.unwrap_or(1) as f32;
        let vw = self
            .view
            .aspect
            .and_then(parse_aspect)
            .map_or(vw, |a| vh * a as f32);
        shown.x / vw
    }

    fn set_zoom(&mut self, zoom: ViewZoom) {
        self.view.zoom = zoom;
        if zoom == ViewZoom::Fit {
            self.view.pan = (0.0, 0.0);
        }
    }

    pub(super) fn viewer(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        let video_rect = egui::Rect::from_min_max(
            full.min,
            egui::pos2(full.max.x, (full.max.y - TRANSPORT_HEIGHT).max(full.min.y)),
        );
        let transport_rect =
            egui::Rect::from_min_max(egui::pos2(full.min.x, video_rect.max.y), full.max);

        let response = ui.allocate_rect(video_rect, Sense::click_and_drag());
        if self.player.file.is_some() {
            let ppp = ui.ctx().pixels_per_point();
            let rect_px = video_rect.size() * ppp;
            self.apply_view(rect_px);
            self.player.paint(ui, video_rect);
            if let Some(preview) = self.timeline.marker_preview {
                self.marker_overlay(ui, video_rect, preview);
            }
            if response.clicked() {
                self.pending.push(Action::PlayPause);
            }
            // Drag (or middle-drag) to pan when the picture is bigger than the viewer.
            if self.view.zoom != ViewZoom::Fit
                && response.dragged()
                && let Some((shown, _)) = self.view_geometry(rect_px)
            {
                let d = response.drag_delta() * ppp;
                let clamp = |v: f64| v.clamp(-0.5, 0.5);
                self.view.pan.0 = clamp(self.view.pan.0 + (d.x / shown.x) as f64);
                self.view.pan.1 = clamp(self.view.pan.1 + (d.y / shown.y) as f64);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if let Some(pointer) = response.hover_pos() {
                let factor = ui.input(|i| i.zoom_delta());
                if factor != 1.0 {
                    self.zoom_view_at(factor, pointer, video_rect, ppp);
                }
            }
            let rc_time = self.player.time;
            response.context_menu(|ui| self.viewer_menu(ui, rc_time));
        } else {
            let p = ui.painter();
            p.text(
                video_rect.center() - vec2(0.0, 12.0),
                Align2::CENTER_CENTER,
                "Drop an episode here",
                FontId::proportional(24.0),
                self.palette.fg_dim,
            );
            let open = self.settings.keymap.text(Action::Open);
            p.text(
                video_rect.center() + vec2(0.0, 18.0),
                Align2::CENTER_CENTER,
                format!("or click to open ({open})  ·  F1 for help"),
                FontId::proportional(14.0),
                self.palette.fg_dim,
            );
            if response.clicked() {
                self.pending.push(Action::Open);
            }
        }

        let mut transport = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(transport_rect.shrink2(vec2(10.0, 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        ui.painter()
            .rect_filled(transport_rect, 0.0, self.palette.bg1);
        self.transport(&mut transport);
    }

    /// While an In/Out marker (or the range) is dragged on the timeline the viewer shows the
    /// marker's frame — make that unmistakable: a marker-colored frame and a label.
    fn marker_overlay(&self, ui: &egui::Ui, rect: egui::Rect, preview: MarkerPreview) {
        let p = self.palette;
        let fps = self.player.fps;
        let painter = ui.painter_at(rect);
        painter.rect_stroke(
            rect.shrink(2.0),
            0.0,
            egui::Stroke::new(4.0, p.marker),
            egui::StrokeKind::Inside,
        );

        let tc = |t: f64| format!("{}  ·  frame {}", fmt_timecode(t, fps), frame_at(t, fps));
        let (title, detail) = match preview {
            MarkerPreview::In(t) => (format!("{}  IN POINT", icon::MARK_IN), tc(t)),
            MarkerPreview::Out(t) => (format!("OUT POINT  {}", icon::MARK_OUT), tc(t)),
            MarkerPreview::Range(a, b) => (
                "MOVING IN → OUT  (showing In)".to_string(),
                format!(
                    "{} → {}  ·  {:.2}s",
                    fmt_timecode(a, fps),
                    fmt_timecode(b, fps),
                    b - a
                ),
            ),
        };
        let back = self.timeline.restore_time().map(|t| {
            format!(
                "Playhead returns to {} when you let go",
                fmt_timecode(t, fps)
            )
        });

        let lines = [
            (title, egui::FontId::proportional(18.0), p.marker),
            (detail, egui::FontId::monospace(14.0), egui::Color32::WHITE),
        ];
        let galleys: Vec<_> = lines
            .into_iter()
            .map(|(text, font, color)| painter.layout_no_wrap(text, font, color))
            .chain(back.map(|text| {
                painter.layout_no_wrap(
                    text,
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_gray(200),
                )
            }))
            .collect();
        let pad = vec2(12.0, 8.0);
        let width = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
        let height: f32 = galleys.iter().map(|g| g.size().y + 2.0).sum();
        let badge = egui::Rect::from_min_size(
            rect.left_top() + vec2(14.0, 14.0),
            vec2(width, height) + pad * 2.0,
        );
        painter.rect_filled(badge, 6.0, egui::Color32::from_black_alpha(190));
        painter.rect_stroke(
            badge,
            6.0,
            egui::Stroke::new(1.5, p.marker),
            egui::StrokeKind::Inside,
        );
        let mut y = badge.top() + pad.y;
        for g in galleys {
            let h = g.size().y;
            painter.galley(egui::pos2(badge.left() + pad.x, y), g, egui::Color32::WHITE);
            y += h + 2.0;
        }
    }

    /// Right-click menu on the preview.
    fn viewer_menu(&mut self, ui: &mut egui::Ui, t: f64) {
        ui.label(
            RichText::new(fmt_timecode(t, self.player.fps))
                .monospace()
                .weak(),
        );
        ui.separator();
        ui.menu_button("Zoom", |ui| {
            let zooms = [
                (ViewZoom::Fit, "Fit"),
                (ViewZoom::Scale(0.5), "50%"),
                (ViewZoom::Scale(1.0), "100% (pixel-exact)"),
                (ViewZoom::Scale(2.0), "200%"),
                (ViewZoom::Fill, "Fill (crop)"),
            ];
            for (z, label) in zooms {
                if ui.radio(self.view.zoom == z, label).clicked() {
                    self.set_zoom(z);
                }
            }
            ui.separator();
            ui.weak("Ctrl+wheel zooms at the pointer; drag to pan");
        });
        ui.menu_button("Aspect ratio", |ui| {
            for &(a, label) in ASPECTS {
                if ui.radio(self.view.aspect == a, label).clicked() {
                    self.view.aspect = a;
                }
            }
        });
        let full = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let label = if full {
            "Exit fullscreen"
        } else {
            "Fullscreen"
        };
        if ui
            .add(
                egui::Button::new(label)
                    .shortcut_text(self.settings.keymap.text(Action::Fullscreen)),
            )
            .clicked()
        {
            self.pending.push(Action::Fullscreen);
        }
        ui.separator();
        let mut subs = self.settings.show_subs_preview;
        if ui.checkbox(&mut subs, "Show subtitles").changed() {
            self.pending.push(Action::ToggleSubtitles);
        }
        ui.menu_button("Audio track", |ui| self.track_list(ui, TrackKind::Audio));
        ui.menu_button("Subtitle track", |ui| self.track_list(ui, TrackKind::Sub));
        ui.separator();
        for (action, label) in [
            (Action::Screenshot, "Screenshot"),
            (Action::ScreenshotClean, "Screenshot without subtitles"),
            (Action::SetIn, "Set In here"),
            (Action::SetOut, "Set Out here"),
            (Action::MarkSubtitleLine, "Mark subtitle line"),
        ] {
            if ui
                .add(egui::Button::new(label).shortcut_text(self.settings.keymap.text(action)))
                .clicked()
            {
                self.pending.push(action);
            }
        }
        if ui.button("Copy timecode").clicked() {
            ui.ctx().copy_text(fmt_time(t));
        }
        ui.separator();
        let mut side = self.settings.show_inspector;
        if ui.checkbox(&mut side, "Side panel").changed() {
            self.settings.show_inspector = side;
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let loaded = self.player.file.is_some();
        ui.add_enabled_ui(loaded, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            self.action_button(ui, Action::GoToIn, icon::SKIP_BACK);
            self.action_button(ui, Action::SeekBackSmall, icon::REWIND);
            self.action_button(ui, Action::FrameBack, icon::CARET_LEFT);
            let play = if self.player.paused {
                icon::PLAY
            } else {
                icon::PAUSE
            };
            let r = ui
                .add_sized([44.0, 28.0], egui::Button::new(filled(play).size(18.0)))
                .on_hover_text(format!(
                    "Play / pause ({})",
                    self.settings.keymap.text(Action::PlayPause)
                ));
            if r.clicked() {
                self.pending.push(Action::PlayPause);
            }
            self.action_button(ui, Action::FrameForward, icon::CARET_RIGHT);
            self.action_button(ui, Action::SeekForwardSmall, icon::FAST_FORWARD);
            self.action_button(ui, Action::GoToOut, icon::SKIP_FORWARD);
            ui.add_space(6.0);
            let p = self.palette;
            match widgets::shuttle(ui, &mut self.shuttle_ui, 150.0, self.shuttle, &p) {
                widgets::ShuttleInput::Held(speed) => {
                    if (speed - self.shuttle).abs() > 1e-6 {
                        self.set_shuttle(speed);
                    }
                }
                widgets::ShuttleInput::Released => self.set_shuttle(0.0),
                widgets::ShuttleInput::Nudge(steps) => self.nudge_shuttle(steps),
                widgets::ShuttleInput::Idle => {}
            }
        });

        ui.add_space(12.0);
        self.timecode_field(ui);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let slider =
                egui::Slider::new(&mut self.settings.volume, 0.0..=100.0).show_value(false);
            if ui.add_sized([90.0, 18.0], slider).changed() {
                self.player.set_volume(self.settings.volume);
            }
            let muted = self.settings.volume <= 0.0;
            let speaker = if muted {
                icon::SPEAKER_OFF
            } else {
                icon::SPEAKER
            };
            if ui
                .small_button(speaker)
                .on_hover_text("Mute / unmute")
                .clicked()
            {
                self.settings.volume = if muted { 70.0 } else { 0.0 };
                self.player.set_volume(self.settings.volume);
            }
            ui.separator();
            ui.add_enabled_ui(loaded, |ui| {
                let mut subs = self.settings.show_subs_preview;
                let tip = format!(
                    "Show subtitles in the preview ({}). Exports aren't affected.",
                    self.settings.keymap.text(Action::ToggleSubtitles)
                );
                if ui
                    .toggle_value(&mut subs, icon::SUBTITLES)
                    .on_hover_text(tip)
                    .changed()
                {
                    self.pending.push(Action::ToggleSubtitles);
                }
                let mut looping = self.looping;
                let loop_tip = format!(
                    "Loop In→Out ({})",
                    self.settings.keymap.text(Action::ToggleLoop)
                );
                if ui
                    .toggle_value(&mut looping, icon::REPEAT)
                    .on_hover_text(loop_tip)
                    .changed()
                {
                    self.pending.push(Action::ToggleLoop);
                }
                self.action_button(ui, Action::SetOut, &format!("Out {}", icon::MARK_OUT));
                self.action_button(ui, Action::SetIn, &format!("{} In", icon::MARK_IN));
            });
        });
    }

    /// Big timecode; click it to type a position (`1:23.5`, `00:01:23:12`, `#2000`).
    fn timecode_field(&mut self, ui: &mut egui::Ui) {
        let fps = self.player.fps;
        let t = self.player.time;
        let mono = FontId::monospace(18.0);

        if let Some(text) = &mut self.ui.timecode_edit {
            let edit = egui::TextEdit::singleline(text)
                .font(mono)
                .desired_width(150.0)
                .hint_text("1:23.5 / #2000");
            let r = ui.add(edit);
            r.request_focus();
            if r.lost_focus() {
                let entered = text.clone();
                self.ui.timecode_edit = None;
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    match parse_position(&entered, fps) {
                        Some(pos) => self.player.seek(pos, true),
                        None => self.error(format!("Couldn't understand position \"{entered}\"")),
                    }
                }
            }
        } else {
            let r = ui
                .add(
                    egui::Label::new(
                        RichText::new(fmt_timecode(t, fps))
                            .font(mono)
                            .color(self.palette.fg),
                    )
                    .sense(Sense::click()),
                )
                .on_hover_text("Click to go to a time, timecode or #frame");
            if r.clicked() {
                self.ui.timecode_edit = Some(fmt_time(t));
            }
        }

        // Two small lines, centered vertically against the big timecode.
        let height = ui.available_height();
        let layout = egui::Layout::top_down(egui::Align::Min).with_main_align(egui::Align::Center);
        ui.allocate_ui_with_layout(vec2(150.0, height), layout, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(
                RichText::new(format!("frame {}", frame_at(t, fps)))
                    .small()
                    .color(self.palette.fg_dim),
            );
            ui.label(
                RichText::new(format!(
                    "{} · {:.3} fps",
                    fmt_time(self.player.duration),
                    fps
                ))
                .small()
                .color(self.palette.fg_dim),
            );
        });
    }
}
