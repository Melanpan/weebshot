//! Zoomable timeline with ruler, video lane (chapters), subtitle lane and In/Out markers.

use eframe::egui::{self, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, pos2, vec2};

use super::App;
use crate::fonts::icon;
use crate::keymap::Action;
use crate::subs::Cue;
use crate::theme::Palette;
use crate::thumbs::{THUMB_H, Thumbs};
use crate::util::{fmt_ruler, fmt_time};
use crate::waveform::Waveform;

const GUTTER: f32 = 58.0;
const RULER_H: f32 = 22.0;
const VIDEO_H: f32 = 58.0;
const SUBS_H: f32 = 26.0;
const AUDIO_H: f32 = 36.0;
/// Smallest visible span (seconds).
const MIN_SPAN: f64 = 0.4;

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Scrub,
    In,
    Out,
    /// Moving the whole In→Out range; `grab` is the time under the pointer at the start.
    Range {
        grab: f64,
        a: f64,
        b: f64,
    },
    Pan {
        start_x: f32,
        start_view: f64,
    },
}

pub struct TimelineState {
    view_start: f64,
    /// `None` = fit the whole file.
    view_len: Option<f64>,
    drag: Option<Drag>,
    /// Position under the pointer while scrubbing.
    pub scrub: Option<f64>,
    /// Playhead position when a drag started (marker drags return to it).
    restore_time: Option<f64>,
    /// What the viewer is previewing while a marker (or the range) is dragged.
    pub marker_preview: Option<MarkerPreview>,
    /// Range and thumbnail spacing the filmstrip wants decoded: (start, end, spacing).
    pub thumb_request: Option<(f64, f64, f64)>,
    last_time: f64,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            view_start: 0.0,
            view_len: None,
            drag: None,
            scrub: None,
            restore_time: None,
            marker_preview: None,
            thumb_request: None,
            last_time: 0.0,
        }
    }
}

impl TimelineState {
    pub fn fit(&mut self) {
        self.view_len = None;
        self.view_start = 0.0;
    }

    fn span(&self, duration: f64) -> f64 {
        self.view_len.unwrap_or(duration).max(MIN_SPAN)
    }

    /// Multiply the visible span by `factor`, keeping `anchor` (default: view center) in place.
    pub fn zoom(&mut self, factor: f64, anchor: Option<f64>, duration: f64) {
        let span = self.span(duration);
        let new = (span * factor).clamp(MIN_SPAN, duration.max(MIN_SPAN));
        let anchor = anchor.unwrap_or(self.view_start + span / 2.0);
        let rel = (anchor - self.view_start) / span;
        self.view_start = anchor - rel * new;
        self.view_len = (new < duration).then_some(new);
        self.clamp(duration);
    }

    pub(super) fn zoom_to(&mut self, a: f64, b: f64, duration: f64) {
        let pad = (b - a) * 0.1;
        self.view_len = Some((b - a + 2.0 * pad).max(MIN_SPAN));
        self.view_start = a - pad;
        self.clamp(duration);
    }

    /// Zoom as 0 (whole file) … 1 (most zoomed in), on a log scale.
    pub fn zoom_level(&self, duration: f64) -> f32 {
        if duration <= MIN_SPAN {
            return 0.0;
        }
        ((duration / self.span(duration)).ln() / (duration / MIN_SPAN).ln()) as f32
    }

    /// Set the zoom from a 0…1 level, keeping `anchor` (e.g. the playhead) in view.
    pub fn set_zoom_level(&mut self, level: f32, anchor: f64, duration: f64) {
        if duration <= MIN_SPAN {
            return;
        }
        let span = duration * (MIN_SPAN / duration).powf(level.clamp(0.0, 1.0) as f64);
        let factor = span / self.span(duration);
        let anchor = if (self.view_start..=self.view_start + self.span(duration)).contains(&anchor)
        {
            anchor
        } else {
            self.view_start + self.span(duration) / 2.0
        };
        self.zoom(factor, Some(anchor), duration);
    }

    fn clamp(&mut self, duration: f64) {
        let span = self.span(duration);
        self.view_start = self.view_start.clamp(0.0, (duration - span).max(0.0));
        if self.view_len.is_none() {
            self.view_start = 0.0;
        }
    }
}

/// A marker being dragged: the viewer shows its frame and labels it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkerPreview {
    In(f64),
    Out(f64),
    /// The whole range is moving: (in, out).
    Range(f64, f64),
}

impl TimelineState {
    /// Show a marker preview without an actual drag (docs screenshots).
    pub(super) fn fake_marker_drag(
        &mut self,
        preview: Option<MarkerPreview>,
        restore: Option<f64>,
    ) {
        self.marker_preview = preview;
        self.restore_time = restore;
    }
}

pub enum Event {
    Seek {
        t: f64,
        exact: bool,
    },
    SetIn(f64),
    SetOut(f64),
    SetRange(f64, f64),
    /// Play from a to b, then stop (Ctrl+click on a subtitle line).
    PlayRange(f64, f64),
    Clear,
}

struct Data<'a> {
    time: f64,
    duration: f64,
    fps: f64,
    in_point: Option<f64>,
    out_point: Option<f64>,
    chapters: &'a [(f64, String)],
    cues: Option<&'a [Cue]>,
    cues_loading: bool,
    has_sub_track: bool,
    show_sub_lane: bool,
    thumbs: Option<&'a Thumbs>,
    waveform: Option<&'a Waveform>,
    show_audio_lane: bool,
    p: Palette,
}

/// Frames per thumbnail slot: the smallest step at least `min_frames` wide.
/// Slots sit on a fixed frame grid, so zooming only re-samples when the step
/// changes instead of on every pixel of zoom.
fn slot_frames(min_frames: f64) -> u64 {
    const STEPS: &[u64] = &[
        1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536,
        2048, 3072, 4096, 6144, 8192,
    ];
    STEPS
        .iter()
        .copied()
        .find(|&n| n as f64 >= min_frames)
        .unwrap_or(8192)
}

/// Thumbnails along the video lane. When a single frame is wider than a thumbnail,
/// every frame gets its own cell (with its frame number).
#[allow(clippy::too_many_arguments)]
fn filmstrip(
    painter: &egui::Painter,
    st: &mut TimelineState,
    d: &Data,
    thumbs: &Thumbs,
    lane: Rect,
    x_of: &dyn Fn(f64) -> f32,
    t_of: &dyn Fn(f32) -> f64,
    px_per_sec: f64,
) {
    if lane.width() <= 0.0 {
        return;
    }
    let h = lane.height();
    let tw = h * thumbs.width as f32 / THUMB_H as f32;
    let frame_px = (px_per_sec / d.fps) as f32;
    let painter = painter.with_clip_rect(lane);
    let n = slot_frames((tw / frame_px) as f64);
    let dt = n as f64 / d.fps;
    let slot_px = dt as f32 * px_per_sec as f32;
    let per_frame = n == 1;

    const FADE: f32 = 0.2;
    let time = painter.ctx().input(|i| i.time) as f32;
    let mut animating = false;
    let first = (t_of(lane.left()) / dt).floor() as i64;
    let last = (t_of(lane.right()) / dt).ceil() as i64;
    for k in first.max(0)..=last {
        let t = k as f64 * dt;
        let slot = Rect::from_min_size(pos2(x_of(t), lane.top()), vec2(slot_px, h));
        let (rect, uv) = if per_frame && slot_px > tw {
            // Wide frame cells: keep the picture's shape, centered.
            (
                Rect::from_center_size(slot.center(), vec2(tw, h)),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            )
        } else {
            // Fill the slot (it's at most ~1.5× a thumbnail wide) by cropping top/bottom.
            let visible = (tw / slot_px).min(1.0) * 0.5;
            (
                slot,
                Rect::from_min_max(pos2(0.0, 0.5 - visible), pos2(1.0, 0.5 + visible)),
            )
        };

        let thumb = thumbs.slot(t, dt);
        let loading = thumbs.loading(t);
        match thumb.exact {
            // The real frame for this slot: fade it in over whatever was there.
            Some((tex, age)) => {
                let alpha = (age / FADE).min(1.0);
                if alpha < 1.0 {
                    animating = true;
                    if let Some(prev) = thumb.before {
                        painter.image(prev.id(), rect, uv, Color32::from_gray(110));
                    }
                }
                painter.image(tex.id(), rect, uv, Color32::WHITE.gamma_multiply(alpha));
            }
            // Not decoded yet: show the nearest earlier frame, dimmed, with a shimmer.
            None if loading => {
                painter.rect_filled(rect, 0.0, d.p.bg2);
                if let Some(prev) = thumb.before {
                    painter.image(prev.id(), rect, uv, Color32::from_gray(90));
                }
                let phase = ((time * 0.8 - k as f32 * 0.07).rem_euclid(1.0) - 0.5).abs() * 2.0;
                painter.rect_filled(
                    rect,
                    0.0,
                    Color32::from_white_alpha((30.0 * (1.0 - phase)) as u8),
                );
                if rect.width() >= 22.0 {
                    let r = (h * 0.16).clamp(5.0, 9.0);
                    painter.circle_filled(rect.center(), r + 3.0, Color32::from_black_alpha(110));
                    super::widgets::spinner_arc(
                        &painter,
                        rect.center(),
                        r,
                        2.0,
                        Color32::WHITE,
                        k as f32 * 0.05,
                    );
                }
                animating = true;
            }
            // Fully loaded and no frame starts inside this slot: the earlier frame is correct.
            None => {
                if let Some(prev) = thumb.before {
                    painter.image(prev.id(), rect, uv, Color32::WHITE);
                }
            }
        }
        painter.vline(slot.left(), lane.y_range(), Stroke::new(1.0, d.p.bg0));
        if per_frame && frame_px >= 34.0 {
            let frame = (t * d.fps).round() as u64;
            painter.text(
                rect.left_bottom() + vec2(3.0, -2.0),
                Align2::LEFT_BOTTOM,
                frame.to_string(),
                FontId::monospace(9.0),
                Color32::WHITE,
            );
        }
    }
    if animating {
        painter.ctx().request_repaint();
    }
    st.thumb_request = Some((t_of(lane.left()), t_of(lane.right()), dt));
}

/// The audio lane: one bar per pixel column, height = loudest peak there.
fn waveform_lane(
    painter: &egui::Painter,
    d: &Data,
    lane: Rect,
    x_of: &dyn Fn(f64) -> f32,
    t_of: &dyn Fn(f32) -> f64,
    px_per_sec: f64,
) {
    let Some(wave) = d.waveform else {
        painter.text(
            pos2(lane.left() + 8.0, lane.center().y),
            Align2::LEFT_CENTER,
            "no audio track selected",
            FontId::proportional(11.0),
            d.p.fg_dim,
        );
        return;
    };
    let painter = painter.with_clip_rect(lane);
    let mid = lane.center().y;
    let half = lane.height() / 2.0 - 3.0;
    let color = d.p.ok.gamma_multiply(0.85);
    painter.hline(
        lane.x_range(),
        mid,
        Stroke::new(1.0, color.gamma_multiply(0.35)),
    );
    let file_end = x_of(d.duration).min(lane.right());
    // At high zoom one peak covers several pixels: step by that instead of by pixel.
    let step = ((px_per_sec / crate::waveform::PEAKS_PER_SEC) as f32).max(1.0);
    let mut missing_from = None;
    let mut x = lane.left();
    while x < file_end {
        let (a, b) = (t_of(x), t_of(x + step));
        match wave.peak(a, b) {
            Some(peak) => {
                // sqrt makes quiet dialogue visible next to loud music.
                let h = (peak.sqrt() * half).max(0.5);
                if step > 1.5 {
                    painter.rect_filled(
                        Rect::from_min_max(pos2(x, mid - h), pos2(x + step - 1.0, mid + h)),
                        0.0,
                        color,
                    );
                } else {
                    painter.vline(x, mid - h..=mid + h, Stroke::new(1.0, color));
                }
            }
            None => {
                missing_from.get_or_insert(x);
            }
        }
        x += step;
    }
    if wave.loading()
        && let Some(x) = missing_from
    {
        painter.text(
            pos2(x + 8.0, mid),
            Align2::LEFT_CENTER,
            "reading audio…",
            FontId::proportional(11.0),
            d.p.fg_dim,
        );
        painter.ctx().request_repaint();
    }
}

/// Pick a ruler step (seconds) so labels are at least `min_px` apart.
fn ruler_step(px_per_sec: f64, fps: f64, min_px: f64) -> (f64, bool) {
    let frame = 1.0 / fps;
    for n in [1.0, 2.0, 5.0, 10.0] {
        if n * frame * px_per_sec >= min_px {
            return (n * frame, true);
        }
    }
    for s in [
        0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0,
    ] {
        if s * px_per_sec >= min_px {
            return (s, false);
        }
    }
    (7200.0, false)
}

fn show(ui: &mut egui::Ui, st: &mut TimelineState, d: &Data) -> Vec<Event> {
    let mut events = Vec::new();
    let p = d.p;
    let audio_h = if d.show_audio_lane { AUDIO_H } else { 0.0 };
    let lanes_h = RULER_H + VIDEO_H + audio_h + if d.show_sub_lane { SUBS_H } else { 0.0 };
    let size = vec2(
        ui.available_width(),
        ui.available_height().max(lanes_h + 4.0),
    );
    let (canvas, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(canvas);

    let track = Rect::from_min_max(pos2(canvas.left() + GUTTER, canvas.top()), canvas.max);
    let duration = d.duration.max(0.001);
    st.clamp(duration);
    let span = st.span(duration);
    let vs = st.view_start;
    let px_per_sec = track.width() as f64 / span;
    let frame_px = px_per_sec / d.fps;
    let x_of = |t: f64| track.left() + ((t - vs) * px_per_sec) as f32;
    let t_of = |x: f32| (vs + (x - track.left()) as f64 / px_per_sec).clamp(0.0, duration);
    // Seek targets snap to the start of the frame under the pointer.
    let snap = |t: f64| ((t * d.fps).floor() / d.fps).clamp(0.0, duration);

    // Follow the playhead when it moves out of view (playback, stepping, seeking).
    let time_moved = (d.time - st.last_time).abs() > 1e-6;
    st.last_time = d.time;
    if st.drag.is_none()
        && st.restore_time.is_none()
        && time_moved
        && (d.time < vs || d.time > vs + span)
    {
        st.view_start = d.time - span * 0.05;
        st.clamp(duration);
    }

    // Rows
    let ruler = Rect::from_min_size(track.min, vec2(track.width(), RULER_H));
    let video = Rect::from_min_size(
        pos2(track.left(), ruler.bottom()),
        vec2(track.width(), VIDEO_H),
    );
    let audio = Rect::from_min_size(
        pos2(track.left(), video.bottom()),
        vec2(track.width(), audio_h),
    );
    let subs = Rect::from_min_size(
        pos2(track.left(), audio.bottom()),
        vec2(track.width(), SUBS_H),
    );
    let content_bottom = if d.show_sub_lane {
        subs.bottom()
    } else {
        audio.bottom()
    };

    painter.rect_filled(canvas, 0.0, p.bg0);
    painter.rect_filled(
        Rect::from_min_max(canvas.min, pos2(track.left(), canvas.bottom())),
        0.0,
        p.bg1,
    );
    painter.rect_filled(ruler, 0.0, p.bg1);
    let label = |y: f32, text: &str| {
        painter.text(
            pos2(canvas.left() + 8.0, y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(11.0),
            p.fg_dim,
        );
    };
    label(video.center().y, "V1");
    if d.show_audio_lane {
        label(audio.center().y, "A1");
        painter.hline(track.x_range(), audio.top(), Stroke::new(1.0, p.bg1));
        waveform_lane(&painter, d, audio, &x_of, &t_of, px_per_sec);
    }
    if d.show_sub_lane {
        label(subs.center().y, "S1");
        painter.hline(track.x_range(), subs.top(), Stroke::new(1.0, p.bg1));
    }

    // Ruler ticks
    let (step, frame_steps) = ruler_step(px_per_sec, d.fps, 90.0);
    let minor = if frame_steps {
        step / step.round().max(1.0)
    } else {
        step / 5.0
    };
    let mut t = (vs / minor).floor() * minor;
    while t <= vs + span {
        let x = x_of(t);
        let major = ((t / step).round() * step - t).abs() < minor * 0.01;
        let h = if major { 9.0 } else { 4.0 };
        painter.vline(
            x,
            ruler.bottom() - h..=ruler.bottom(),
            Stroke::new(1.0, p.fg_dim),
        );
        if major {
            // Zoomed to individual frames: label frame numbers.
            let text = if frame_steps {
                format!("f{}", (t * d.fps).round() as u64)
            } else {
                fmt_ruler(t, step < 1.0)
            };
            painter.text(
                pos2(x + 3.0, ruler.top() + 2.0),
                Align2::LEFT_TOP,
                text,
                FontId::monospace(10.0),
                p.fg_dim,
            );
        }
        t += minor;
    }
    // Frame grid when zoomed in far enough to see individual frames.
    if frame_px >= 10.0 {
        let mut f = (vs * d.fps).floor();
        while f / d.fps <= vs + span {
            let x = x_of(f / d.fps);
            painter.vline(x, ruler.bottom()..=content_bottom, Stroke::new(1.0, p.bg1));
            f += 1.0;
        }
    }

    // Video lane
    let file_rect =
        Rect::from_x_y_ranges(x_of(0.0)..=x_of(duration), video.y_range()).shrink2(vec2(0.0, 4.0));
    painter.rect_filled(
        file_rect.intersect(track),
        3.0,
        p.accent.gamma_multiply(0.30),
    );
    st.thumb_request = None;
    if let Some(thumbs) = d.thumbs {
        filmstrip(
            &painter,
            st,
            d,
            thumbs,
            file_rect.intersect(track),
            &x_of,
            &t_of,
            px_per_sec,
        );
    }
    for (t, title) in d.chapters {
        let x = x_of(*t);
        if track.x_range().contains(x) {
            painter.vline(x, video.y_range(), Stroke::new(2.0, p.chapter));
            painter.text(
                pos2(x + 4.0, video.top() + 5.0),
                Align2::LEFT_TOP,
                title,
                FontId::proportional(11.0),
                p.chapter,
            );
        }
    }

    // Subtitle lane
    let mut hovered_cue: Option<&Cue> = None;
    let hover = resp.hover_pos();
    if d.show_sub_lane {
        match d.cues {
            Some(cues) => {
                for cue in cues.iter().filter(|c| c.end >= vs && c.start <= vs + span) {
                    let mut r =
                        Rect::from_x_y_ranges(x_of(cue.start)..=x_of(cue.end), subs.y_range())
                            .shrink2(vec2(0.0, 3.0));
                    if r.width() < 2.0 {
                        r = r.expand2(vec2(1.0, 0.0));
                    }
                    let is_hovered = hover.is_some_and(|h| r.contains(h));
                    if is_hovered {
                        hovered_cue = Some(cue);
                    }
                    let fill = p
                        .subtitle
                        .gamma_multiply(if is_hovered { 0.9 } else { 0.55 });
                    painter.rect_filled(r, 2.0, fill);
                    if r.width() > 28.0 {
                        let text_painter = painter.with_clip_rect(r.shrink(2.0).intersect(track));
                        text_painter.text(
                            pos2(r.left() + 4.0, r.center().y),
                            Align2::LEFT_CENTER,
                            &cue.text,
                            FontId::proportional(11.0),
                            if p.dark {
                                Color32::BLACK
                            } else {
                                Color32::WHITE
                            },
                        );
                    }
                }
            }
            None => {
                let msg = if d.cues_loading {
                    "reading subtitles…"
                } else if d.has_sub_track {
                    "subtitle timing unavailable for this track"
                } else {
                    "no subtitle track selected"
                };
                painter.text(
                    pos2(track.left() + 8.0, subs.center().y),
                    Align2::LEFT_CENTER,
                    msg,
                    FontId::proportional(11.0),
                    p.fg_dim,
                );
            }
        }
    }

    // In/Out range
    if d.in_point.is_some() || d.out_point.is_some() {
        let a = d.in_point.unwrap_or(0.0);
        let b = d.out_point.unwrap_or(duration);
        let range =
            Rect::from_x_y_ranges(x_of(a)..=x_of(b), ruler.top()..=content_bottom).intersect(track);
        painter.rect_filled(range, 0.0, p.marker.gamma_multiply(0.10));
        let ruler_band =
            Rect::from_x_y_ranges(range.x_range(), ruler.bottom() - 4.0..=ruler.bottom());
        painter.rect_filled(ruler_band, 0.0, p.marker.gamma_multiply(0.8));
    }
    for (t, tag) in [(d.in_point, "I"), (d.out_point, "O")] {
        let Some(t) = t else { continue };
        let x = x_of(t);
        if !track.x_range().contains(x) {
            continue;
        }
        painter.vline(x, ruler.top()..=content_bottom, Stroke::new(1.5, p.marker));
        let flag = if tag == "I" {
            Rect::from_min_size(pos2(x, ruler.top()), vec2(12.0, 12.0))
        } else {
            Rect::from_min_size(pos2(x - 12.0, ruler.top()), vec2(12.0, 12.0))
        };
        painter.rect_filled(flag, 1.0, p.marker);
        painter.text(
            flag.center(),
            Align2::CENTER_CENTER,
            tag,
            FontId::monospace(9.0),
            Color32::BLACK,
        );
    }

    // Hover line
    if let Some(h) = hover
        && track.contains(h)
        && st.drag.is_none()
    {
        painter.vline(
            h.x,
            ruler.top()..=content_bottom,
            Stroke::new(1.0, p.fg_dim.gamma_multiply(0.5)),
        );
    }

    // Playhead
    let head_t = match st.drag {
        Some(Drag::In | Drag::Out | Drag::Range { .. }) => st.restore_time.unwrap_or(d.time),
        _ => st.scrub.unwrap_or(d.time),
    };
    let hx = x_of(head_t);
    if track.x_range().contains(hx) {
        painter.vline(
            hx,
            ruler.top()..=canvas.bottom(),
            Stroke::new(2.0, p.playhead),
        );
        let head = [
            pos2(hx - 6.0, ruler.top()),
            pos2(hx + 6.0, ruler.top()),
            pos2(hx + 6.0, ruler.top() + 8.0),
            pos2(hx, ruler.top() + 14.0),
            pos2(hx - 6.0, ruler.top() + 8.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            head.to_vec(),
            p.playhead,
            Stroke::NONE,
        ));
    }

    // ---- interaction ----------------------------------------------------
    let near_in = |x: f32| {
        d.in_point
            .is_some_and(|t| (-6.0..=14.0).contains(&(x - x_of(t))))
    };
    let near_out = |x: f32| {
        d.out_point
            .is_some_and(|t| (-14.0..=6.0).contains(&(x - x_of(t))))
    };
    let precise = frame_px >= 3.0;
    let frame = 1.0 / d.fps;
    let range = (d.in_point.is_some() || d.out_point.is_some())
        .then(|| (d.in_point.unwrap_or(0.0), d.out_point.unwrap_or(duration)));
    let in_range_band = |pos: egui::Pos2| {
        range.is_some_and(|(a, b)| {
            pos.y <= ruler.bottom() && pos.x > x_of(a) + 6.0 && pos.x < x_of(b) - 6.0
        })
    };
    // Marker edges snap to subtitle line boundaries and chapters (hold Shift to disable).
    let no_snap = ui.input(|i| i.modifiers.shift);
    let snap_edge = |t: f64| -> f64 {
        if no_snap {
            return t;
        }
        let x = x_of(t);
        let candidates = d
            .cues
            .unwrap_or(&[])
            .iter()
            .filter(|c| c.end >= vs && c.start <= vs + span)
            .flat_map(|c| [c.start, c.end])
            .chain(d.chapters.iter().map(|(t, _)| *t));
        candidates
            .map(|c| (c, (x_of(c) - x).abs()))
            .filter(|(_, dx)| *dx <= 7.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(t, |(c, _)| c)
    };

    // Cursor feedback for the grabbable parts.
    if st.drag.is_none()
        && let Some(h) = hover
        && track.contains(h)
    {
        if near_in(h.x) || near_out(h.x) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        } else if in_range_band(h) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    if resp.drag_started()
        && let Some(pos) = ui
            .input(|i| i.pointer.press_origin())
            .or(resp.interact_pointer_pos())
    {
        let middle = ui.input(|i| i.pointer.middle_down());
        st.restore_time = Some(d.time);
        st.drag = Some(if middle || ui.input(|i| i.modifiers.alt) {
            Drag::Pan {
                start_x: pos.x,
                start_view: st.view_start,
            }
        } else if near_in(pos.x) && (!near_out(pos.x) || pos.x >= x_of(d.in_point.unwrap())) {
            Drag::In
        } else if near_out(pos.x) {
            Drag::Out
        } else if in_range_band(pos) {
            let (a, b) = range.unwrap();
            Drag::Range {
                grab: t_of(pos.x),
                a,
                b,
            }
        } else {
            Drag::Scrub
        });
    }
    if resp.dragged()
        && let (Some(drag), Some(pos)) = (st.drag, resp.interact_pointer_pos())
    {
        let t = snap(t_of(pos.x));
        match drag {
            Drag::Scrub => {
                st.scrub = Some(t);
                events.push(Event::Seek { t, exact: precise });
            }
            Drag::In => {
                let limit = d.out_point.map_or(duration, |o| o - frame);
                let t = snap_edge(t).min(limit);
                st.marker_preview = Some(MarkerPreview::In(t));
                events.push(Event::SetIn(t));
                events.push(Event::Seek { t, exact: precise });
            }
            Drag::Out => {
                let limit = d.in_point.map_or(0.0, |i| i + frame);
                let t = snap_edge(t).max(limit);
                st.marker_preview = Some(MarkerPreview::Out(t));
                events.push(Event::SetOut(t));
                events.push(Event::Seek { t, exact: precise });
            }
            Drag::Range { grab, a, b } => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                let len = b - a;
                let mut start =
                    snap(a + (t_of(pos.x) - grab)).clamp(0.0, (duration - len).max(0.0));
                // Snap whichever edge is close to something.
                let snapped_start = snap_edge(start);
                let snapped_end = snap_edge(start + len) - len;
                if snapped_start != start {
                    start = snapped_start;
                } else if (snapped_end - start).abs() > 1e-9 {
                    start = snapped_end;
                }
                st.marker_preview = Some(MarkerPreview::Range(start, start + len));
                events.push(Event::SetRange(start, start + len));
                events.push(Event::Seek {
                    t: start,
                    exact: precise,
                });
            }
            Drag::Pan {
                start_x,
                start_view,
            } => {
                st.view_start = start_view - (pos.x - start_x) as f64 / px_per_sec;
                st.clamp(duration);
            }
        }
    }
    if resp.drag_stopped() {
        match (st.drag, st.restore_time) {
            (Some(Drag::Scrub), _) => {
                if let Some(t) = resp.interact_pointer_pos().map(|p| snap(t_of(p.x))) {
                    events.push(Event::Seek { t, exact: true });
                }
            }
            // The preview showed the marker's frame while dragging; go back.
            (Some(Drag::In | Drag::Out | Drag::Range { .. }), Some(t)) => {
                events.push(Event::Seek { t, exact: true });
            }
            _ => {}
        }
        st.drag = None;
        st.scrub = None;
        st.restore_time = None;
        st.marker_preview = None;
    }

    let ctrl = ui.input(|i| i.modifiers.command);
    if resp.double_clicked() {
        if let Some(cue) = hovered_cue {
            events.push(Event::SetRange(cue.start, cue.end));
        }
    } else if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && track.contains(pos)
    {
        match hovered_cue {
            Some(cue) if ctrl => events.push(Event::PlayRange(cue.start, cue.end)),
            Some(cue) => events.push(Event::Seek {
                t: cue.start,
                exact: true,
            }),
            None => events.push(Event::Seek {
                t: snap(t_of(pos.x)),
                exact: true,
            }),
        }
    }

    // Ctrl+wheel zooms around the pointer (egui reports it as a zoom gesture, which also
    // covers trackpad pinch); the plain wheel scrolls left/right.
    if let Some(h) = hover
        && track.contains(h)
    {
        let (delta, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if zoom != 1.0 {
            st.zoom(1.0 / zoom as f64, Some(t_of(h.x)), duration);
        } else {
            let pan_px = delta.x + delta.y;
            if pan_px != 0.0 {
                st.view_start -= pan_px as f64 / px_per_sec;
                st.clamp(duration);
            }
        }
    }

    // One tooltip with a fixed width: the time under the pointer, plus the subtitle line.
    if st.drag.is_none()
        && let Some(h) = hover
        && track.contains(h)
    {
        let t = t_of(h.x);
        resp.clone().on_hover_ui_at_pointer(|ui| {
            ui.set_width(300.0);
            ui.label(
                RichText::new(format!("{}  ·  f{}", fmt_time(t), (t * d.fps).floor() as u64))
                    .monospace()
                    .small(),
            );
            if let Some(cue) = hovered_cue {
                ui.add_space(2.0);
                ui.add(egui::Label::new(RichText::new(&cue.text).strong()).wrap());
                ui.label(
                    RichText::new(format!(
                        "{} → {}  ({:.2}s)",
                        fmt_time(cue.start),
                        fmt_time(cue.end),
                        cue.end - cue.start
                    ))
                    .weak()
                    .small(),
                );
                ui.add_space(2.0);
                ui.label(
                    RichText::new(
                        "Click: go to line\nDouble-click: mark as In/Out\nCtrl+click: play just this line",
                    )
                    .weak()
                    .small(),
                );
            } else if near_in(h.x) || near_out(h.x) {
                ui.label(RichText::new("Drag to move (hold Shift to not snap)").weak().small());
            } else if in_range_band(h) {
                ui.label(RichText::new("Drag to move the whole range").weak().small());
            }
        });
    }

    // Right-click menu
    let click_t = ui
        .ctx()
        .pointer_interact_pos()
        .filter(|p| track.contains(*p))
        .map(|p| snap(t_of(p.x)));
    let menu_t_id = ui.scope_id().with("timeline_menu_t");
    if resp.secondary_clicked()
        && let Some(t) = click_t
    {
        ui.data_mut(|data| data.insert_temp(menu_t_id, t));
    }
    resp.context_menu(|ui| {
        let t = ui
            .data(|data| data.get_temp::<f64>(menu_t_id))
            .unwrap_or(d.time);
        ui.label(RichText::new(fmt_time(t)).weak());
        if ui.button("Set In here").clicked() {
            events.push(Event::SetIn(t));
        }
        if ui.button("Set Out here").clicked() {
            events.push(Event::SetOut(t));
        }
        if let Some(cue) = hovered_cue.or_else(|| {
            d.cues
                .and_then(|c| c.iter().find(|c| c.start <= t && t < c.end))
        }) && ui.button("Mark this subtitle line").clicked()
        {
            events.push(Event::SetRange(cue.start, cue.end));
        }
        if ui.button("Clear In/Out").clicked() {
            events.push(Event::Clear);
        }
        ui.separator();
        if let (Some(a), Some(b)) = (d.in_point, d.out_point)
            && ui.button("Zoom to In/Out").clicked()
        {
            st.zoom_to(a, b, duration);
        }
        if ui.button("Fit whole file").clicked() {
            st.fit();
        }
    });

    events
}

impl App {
    pub(super) fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        self.timeline_rect = ui.max_rect();
        let p = self.palette;
        let duration = self.player.duration;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Timeline").strong());
            ui.separator();
            self.action_button(ui, Action::ZoomOut, icon::ZOOM_OUT);
            self.action_button(ui, Action::ZoomIn, icon::ZOOM_IN);
            self.action_button(ui, Action::ZoomFit, icon::FIT);
            ui.separator();
            ui.checkbox(&mut self.settings.show_audio_lane, "Audio");
            ui.checkbox(&mut self.settings.show_sub_lane, "Subtitles");
            if self.cues.as_ref().is_some_and(|c| c.loading()) {
                ui.add(egui::Spinner::new().size(12.0));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.has_range() {
                    let (a, b) = self.clip_range();
                    ui.label(
                        RichText::new(format!("In {}  ·  Out {}  ·  {:.3}s", fmt_time(a), fmt_time(b), b - a))
                            .color(p.marker)
                            .monospace(),
                    );
                } else {
                    let i = self.settings.keymap.text(Action::SetIn);
                    let o = self.settings.keymap.text(Action::SetOut);
                    let x = self.settings.keymap.text(Action::MarkSubtitleLine);
                    ui.weak(format!(
                        "{i}/{o} set In/Out · {x} marks the current subtitle line · wheel zooms · Alt-drag pans"
                    ));
                }
            });
        });

        if self.player.file.is_none() {
            ui.centered_and_justified(|ui| ui.weak("No video"));
            return;
        }

        let data = Data {
            time: self.player.time,
            duration,
            fps: self.player.fps,
            in_point: self.in_point,
            out_point: self.out_point,
            chapters: &self.player.chapters,
            cues: self
                .cues
                .as_ref()
                .filter(|c| !c.loading())
                .map(|c| c.cues.as_slice()),
            cues_loading: self.cues.as_ref().is_some_and(|c| c.loading()),
            has_sub_track: self.player.sid.is_some(),
            show_sub_lane: self.settings.show_sub_lane,
            thumbs: self.thumbs.as_ref(),
            waveform: self.waveform.as_ref(),
            show_audio_lane: self.settings.show_audio_lane,
            p,
        };
        let events = show(ui, &mut self.timeline, &data);
        if let (Some(thumbs), Some((a, b, spacing))) =
            (&mut self.thumbs, self.timeline.thumb_request)
        {
            thumbs.request(a, b, spacing);
        }
        for ev in events {
            match ev {
                Event::Seek { t, exact } => self.player.seek(t, exact),
                Event::SetIn(t) => self.set_in_at(t),
                Event::SetOut(t) => self.set_out_at(t),
                Event::SetRange(a, b) => self.set_range(a, b),
                Event::PlayRange(a, b) => self.play_range(a, b),
                Event::Clear => self.pending.push(Action::ClearInOut),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_steps() {
        // Whole 24 min episode in 1200px: 1 px ≈ 1.2 s → 2 min labels.
        assert_eq!(ruler_step(1200.0 / 1440.0, 23.976, 90.0), (120.0, false));
        // Very zoomed in: individual frames.
        assert!(ruler_step(4000.0, 23.976, 90.0).1);
    }

    #[test]
    fn zoom_keeps_anchor() {
        let mut st = TimelineState::default();
        st.zoom(0.5, Some(600.0), 1440.0);
        assert_eq!(st.view_len, Some(720.0));
        assert!((st.view_start - 300.0).abs() < 1e-9);
        st.zoom(4.0, None, 1440.0);
        assert_eq!(st.view_len, None, "zooming past the file length fits");
    }
}
