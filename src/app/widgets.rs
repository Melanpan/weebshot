//! Custom widgets: progress ring and shuttle.

use std::f32::consts::TAU;

use eframe::egui::{self, Color32, Pos2, Sense, Shape, Stroke, vec2};

use crate::theme::Palette;

fn arc_points(center: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let n = (((to - from).abs() / TAU) * 48.0).ceil().max(2.0) as usize;
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            center + radius * vec2(a.cos(), a.sin())
        })
        .collect()
}

/// A rotating arc, for "busy" states. `phase` offsets the rotation (e.g. per thumbnail).
pub fn spinner_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    width: f32,
    color: Color32,
    phase: f32,
) {
    let t = painter.ctx().input(|i| i.time) as f32;
    let a = -TAU / 4.0 + (t + phase) * TAU * 0.8;
    painter.add(Shape::line(
        arc_points(center, radius, a, a + TAU * 0.3),
        Stroke::new(width, color),
    ));
    painter.ctx().request_repaint();
}

/// Circular progress indicator. `None` spins (unknown progress).
pub fn progress_ring(
    ui: &mut egui::Ui,
    progress: Option<f32>,
    size: f32,
    fg: Color32,
    bg: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let c = rect.center();
    let r = size * 0.5 - 2.0;
    let stroke = (size * 0.14).max(2.0);
    let p = ui.painter();
    p.circle_stroke(c, r, Stroke::new(stroke, bg));
    let start = -TAU / 4.0;
    match progress {
        Some(f) => {
            let f = f.clamp(0.0, 1.0);
            if f > 0.0 {
                p.add(Shape::line(
                    arc_points(c, r, start, start + TAU * f),
                    Stroke::new(stroke, fg),
                ));
            }
        }
        None => spinner_arc(p, c, r, stroke, fg, 0.0),
    }
    resp
}

/// Spring position of the shuttle control (-1 … 1).
#[derive(Default)]
pub struct ShuttleState {
    offset: f32,
    /// Scroll not yet turned into speed steps.
    scroll: f32,
}

/// What the shuttle control is asking for this frame.
pub enum ShuttleInput {
    Idle,
    /// Being held: play at this speed (negative = backwards, 0 = dead zone).
    Held(f64),
    /// Let go this frame.
    Released,
    /// Mouse wheel over the control: go this many speed steps faster (+) / slower (-).
    Nudge(i32),
}

/// Speeds the mouse wheel steps through.
pub const SHUTTLE_STEPS: &[f64] = &[
    -16.0, -8.0, -4.0, -2.0, -1.0, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0,
];

/// Knob position for a speed (inverse of `shuttle_speed`).
fn shuttle_offset(speed: f64) -> f32 {
    if speed == 0.0 {
        return 0.0;
    }
    let t = ((speed.abs() / 0.25).log2() / 6.0).clamp(0.0, 1.0) as f32;
    (DEAD_ZONE + t * (1.0 - DEAD_ZONE)) * speed.signum() as f32
}

/// Dead zone around the center, as a fraction of the half-width.
const DEAD_ZONE: f32 = 0.06;
/// Speed at full deflection.
pub const MAX_SHUTTLE_SPEED: f64 = 16.0;

/// Speed for a deflection: ¼× just outside the dead zone, doubling every sixth of the
/// travel, up to 16× at the end. Negative deflection plays backwards.
pub fn shuttle_speed(offset: f32) -> f64 {
    let mag = offset.abs();
    if mag < DEAD_ZONE {
        return 0.0;
    }
    let t = ((mag - DEAD_ZONE) / (1.0 - DEAD_ZONE)).min(1.0) as f64;
    let speed = 0.25 * 2f64.powf(t * 6.0);
    speed.min(MAX_SHUTTLE_SPEED) * offset.signum() as f64
}

/// Premiere-style shuttle: drag the knob right to play faster, left to play backwards
/// faster; it springs back to the center (stop) when released. `speed` is the current
/// shuttle speed (also set from the keyboard) and is shown on the control.
pub fn shuttle(
    ui: &mut egui::Ui,
    state: &mut ShuttleState,
    width: f32,
    speed: f64,
    p: &Palette,
) -> ShuttleInput {
    let height = 22.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let half = (rect.width() - height) / 2.0;
    let center = rect.center();

    let mut input = ShuttleInput::Idle;
    if resp.dragged()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        state.offset = ((pos.x - center.x) / half).clamp(-1.0, 1.0);
        input = ShuttleInput::Held(shuttle_speed(state.offset));
    } else if resp.drag_stopped() {
        input = ShuttleInput::Released;
    } else {
        // Glide to the current speed (springs back to the center when stopped).
        let target = shuttle_offset(speed);
        if (state.offset - target).abs() > 0.005 {
            state.offset += (target - state.offset) * 0.45;
            ui.ctx().request_repaint();
        } else {
            state.offset = target;
        }
    }
    if resp.hovered() && !resp.dragged() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
        state.scroll += scroll;
        let steps = (state.scroll / 40.0).trunc();
        if steps != 0.0 {
            state.scroll -= steps * 40.0;
            input = ShuttleInput::Nudge(steps as i32);
        }
    } else {
        state.scroll = 0.0;
    }

    let painter = ui.painter();
    painter.rect_filled(rect, height / 2.0, p.bg0);
    // Ticks at 1×, 2×, 4×, 8× on both sides.
    for s in [1.0f64, 2.0, 4.0, 8.0] {
        let frac = DEAD_ZONE + (s / 0.25).log2() as f32 / 6.0 * (1.0 - DEAD_ZONE);
        for sign in [-1.0, 1.0] {
            let x = center.x + sign * frac * half;
            painter.vline(
                x,
                center.y - 3.0..=center.y + 3.0,
                Stroke::new(1.0, p.fg_dim.gamma_multiply(0.6)),
            );
        }
    }
    painter.vline(
        center.x,
        rect.top() + 3.0..=rect.bottom() - 3.0,
        Stroke::new(1.0, p.fg_dim),
    );

    let knob_x = center.x + state.offset * half;
    let knob = egui::Rect::from_center_size(
        egui::pos2(knob_x, center.y),
        vec2(height + 6.0, height - 4.0),
    );
    let active = speed != 0.0 || resp.dragged();
    painter.rect_filled(knob, 6.0, if active { p.accent } else { p.bg3 });
    if speed != 0.0 {
        let arrows = if speed < 0.0 { "◀◀" } else { "▶▶" };
        // Label on the side away from the knob.
        let (anchor, align) = if speed > 0.0 {
            (
                egui::pos2(rect.left() + 8.0, center.y),
                egui::Align2::LEFT_CENTER,
            )
        } else {
            (
                egui::pos2(rect.right() - 8.0, center.y),
                egui::Align2::RIGHT_CENTER,
            )
        };
        painter.text(
            anchor,
            align,
            format!("{arrows} {:.2}×", speed.abs()),
            egui::FontId::monospace(10.0),
            p.fg,
        );
    }
    resp.on_hover_text(
        "Shuttle: drag right to play faster, left to play backwards; let go to stop.\nScroll over it to change speed in steps. Keys: J / K / L.",
    );
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuttle_curve() {
        assert_eq!(shuttle_speed(0.0), 0.0);
        assert_eq!(shuttle_speed(0.03), 0.0);
        assert!((shuttle_speed(DEAD_ZONE) - 0.25).abs() < 1e-6);
        assert!((shuttle_speed(1.0) - 16.0).abs() < 1e-6);
        assert!((shuttle_speed(-1.0) + 16.0).abs() < 1e-6);
        // 1× sits a third of the way out.
        let one = DEAD_ZONE + (1.0 - DEAD_ZONE) / 3.0;
        assert!((shuttle_speed(one) - 1.0).abs() < 1e-3);
    }
}
