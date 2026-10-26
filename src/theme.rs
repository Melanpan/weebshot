//! Color themes.

use eframe::egui::{self, Color32, CornerRadius, Stroke};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Studio,
    GruvboxDark,
    GruvboxLight,
    Nord,
    CatppuccinMocha,
    Dracula,
    Light,
}

/// Colors the custom widgets (timeline, viewer) draw with.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    /// Deepest background (viewer surround, timeline lanes).
    pub bg0: Color32,
    /// Panels.
    pub bg1: Color32,
    /// Widgets.
    pub bg2: Color32,
    /// Hovered widgets.
    pub bg3: Color32,
    pub fg: Color32,
    pub fg_dim: Color32,
    pub accent: Color32,
    pub playhead: Color32,
    pub marker: Color32,
    pub subtitle: Color32,
    pub chapter: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Theme {
    pub const ALL: [Self; 7] = [
        Self::Studio,
        Self::GruvboxDark,
        Self::GruvboxLight,
        Self::Nord,
        Self::CatppuccinMocha,
        Self::Dracula,
        Self::Light,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Studio => "Studio (dark)",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::GruvboxLight => "Gruvbox Light",
            Self::Nord => "Nord",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::Dracula => "Dracula",
            Self::Light => "Light",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Self::Studio => Palette {
                dark: true,
                bg0: hex(0x161616),
                bg1: hex(0x232323),
                bg2: hex(0x313131),
                bg3: hex(0x3d3d3d),
                fg: hex(0xd8d8d8),
                fg_dim: hex(0x8c8c8c),
                accent: hex(0x3d8fd1),
                playhead: hex(0xe5453a),
                marker: hex(0xf0b43c),
                subtitle: hex(0x4f9d8a),
                chapter: hex(0x9a7bd0),
                ok: hex(0x6cc070),
                warn: hex(0xf0b43c),
                error: hex(0xe5554a),
            },
            Self::GruvboxDark => Palette {
                dark: true,
                bg0: hex(0x1d2021),
                bg1: hex(0x282828),
                bg2: hex(0x3c3836),
                bg3: hex(0x504945),
                fg: hex(0xebdbb2),
                fg_dim: hex(0xa89984),
                accent: hex(0x83a598),
                playhead: hex(0xfb4934),
                marker: hex(0xfabd2f),
                subtitle: hex(0x8ec07c),
                chapter: hex(0xd3869b),
                ok: hex(0xb8bb26),
                warn: hex(0xfe8019),
                error: hex(0xfb4934),
            },
            Self::GruvboxLight => Palette {
                dark: false,
                bg0: hex(0xebdbb2),
                bg1: hex(0xfbf1c7),
                bg2: hex(0xebdbb2),
                bg3: hex(0xd5c4a1),
                fg: hex(0x3c3836),
                fg_dim: hex(0x7c6f64),
                accent: hex(0x076678),
                playhead: hex(0x9d0006),
                marker: hex(0xb57614),
                subtitle: hex(0x427b58),
                chapter: hex(0x8f3f71),
                ok: hex(0x79740e),
                warn: hex(0xaf3a03),
                error: hex(0x9d0006),
            },
            Self::Nord => Palette {
                dark: true,
                bg0: hex(0x242933),
                bg1: hex(0x2e3440),
                bg2: hex(0x3b4252),
                bg3: hex(0x434c5e),
                fg: hex(0xe5e9f0),
                fg_dim: hex(0x8f9bb3),
                accent: hex(0x88c0d0),
                playhead: hex(0xbf616a),
                marker: hex(0xebcb8b),
                subtitle: hex(0xa3be8c),
                chapter: hex(0xb48ead),
                ok: hex(0xa3be8c),
                warn: hex(0xd08770),
                error: hex(0xbf616a),
            },
            Self::CatppuccinMocha => Palette {
                dark: true,
                bg0: hex(0x11111b),
                bg1: hex(0x1e1e2e),
                bg2: hex(0x313244),
                bg3: hex(0x45475a),
                fg: hex(0xcdd6f4),
                fg_dim: hex(0xa6adc8),
                accent: hex(0xcba6f7),
                playhead: hex(0xf38ba8),
                marker: hex(0xf9e2af),
                subtitle: hex(0x94e2d5),
                chapter: hex(0xfab387),
                ok: hex(0xa6e3a1),
                warn: hex(0xfab387),
                error: hex(0xf38ba8),
            },
            Self::Dracula => Palette {
                dark: true,
                bg0: hex(0x1e1f29),
                bg1: hex(0x282a36),
                bg2: hex(0x343746),
                bg3: hex(0x44475a),
                fg: hex(0xf8f8f2),
                fg_dim: hex(0x9aa0c4),
                accent: hex(0xbd93f9),
                playhead: hex(0xff5555),
                marker: hex(0xf1fa8c),
                subtitle: hex(0x50fa7b),
                chapter: hex(0xff79c6),
                ok: hex(0x50fa7b),
                warn: hex(0xffb86c),
                error: hex(0xff5555),
            },
            Self::Light => Palette {
                dark: false,
                bg0: hex(0xdcdcdc),
                bg1: hex(0xf3f3f3),
                bg2: hex(0xe2e2e2),
                bg3: hex(0xd0d0d0),
                fg: hex(0x202020),
                fg_dim: hex(0x6a6a6a),
                accent: hex(0x2a6fb8),
                playhead: hex(0xd0302a),
                marker: hex(0xc07d00),
                subtitle: hex(0x2f8a73),
                chapter: hex(0x7a52b8),
                ok: hex(0x2f8a3a),
                warn: hex(0xb36b00),
                error: hex(0xc0302a),
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let p = self.palette();
        let mut v = if p.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = p.bg1;
        v.window_fill = p.bg1;
        v.faint_bg_color = p.bg2.gamma_multiply(0.5);
        v.extreme_bg_color = p.bg0;
        v.code_bg_color = p.bg2;
        v.window_stroke = Stroke::new(1.0, p.bg3);
        v.hyperlink_color = p.accent;
        v.warn_fg_color = p.warn;
        v.error_fg_color = p.error;
        v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.55 } else { 0.35 });
        v.selection.stroke = Stroke::new(1.0, p.fg);

        let radius = CornerRadius::same(3);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = p.bg1;
        w.noninteractive.weak_bg_fill = p.bg1;
        w.noninteractive.bg_stroke = Stroke::new(1.0, p.bg3);
        w.noninteractive.fg_stroke = Stroke::new(1.0, p.fg);
        for (state, bg) in [
            (&mut w.inactive, p.bg2),
            (&mut w.hovered, p.bg3),
            (&mut w.active, p.bg3),
            (&mut w.open, p.bg2),
        ] {
            state.bg_fill = bg;
            state.weak_bg_fill = bg;
            state.corner_radius = radius;
            state.fg_stroke = Stroke::new(1.0, p.fg);
        }
        w.inactive.bg_stroke = Stroke::NONE;
        w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.7));
        w.active.bg_stroke = Stroke::new(1.0, p.accent);
        w.inactive.fg_stroke = Stroke::new(1.0, p.fg.gamma_multiply(0.9));

        ctx.set_visuals_of(egui::Theme::Dark, v.clone());
        ctx.set_visuals_of(egui::Theme::Light, v);
    }
}
