//! Fonts: egui's defaults plus the Phosphor icon font (MIT, assets/fonts).

use std::sync::Arc;

use eframe::egui::{self, FontData, FontFamily, FontTweak, RichText};

pub const FILL_FAMILY: &str = "phosphor-fill";

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let tweak = FontTweak {
        scale: 1.1,
        y_offset_factor: 0.08,
        ..Default::default()
    };
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(
            FontData::from_static(include_bytes!("../assets/fonts/Phosphor.ttf"))
                .tweak(tweak.clone()),
        ),
    );
    fonts.font_data.insert(
        "phosphor-fill".into(),
        Arc::new(
            FontData::from_static(include_bytes!("../assets/fonts/Phosphor-Fill.ttf")).tweak(tweak),
        ),
    );
    // Icons live in the Private Use Area, so they never shadow normal text.
    // Hack covers arrows (←→↑↓) that the proportional font lacks.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        list.push("phosphor".into());
        if !list.iter().any(|f| f == "Hack") {
            list.push("Hack".into());
        }
    }
    fonts.families.insert(
        FontFamily::Name(FILL_FAMILY.into()),
        vec!["phosphor-fill".into(), "Ubuntu-Light".into()],
    );
    ctx.set_fonts(fonts);
}

/// Filled variant of an icon (for prominent buttons like play).
pub fn filled(icon: &str) -> RichText {
    RichText::new(icon).family(FontFamily::Name(FILL_FAMILY.into()))
}

/// Phosphor code points (see assets/fonts/PHOSPHOR-LICENSE.txt).
pub mod icon {
    pub const PLAY: &str = "\u{e3d0}";
    pub const PAUSE: &str = "\u{e39e}";
    pub const SKIP_BACK: &str = "\u{e5a4}";
    pub const SKIP_FORWARD: &str = "\u{e5a6}";
    pub const REWIND: &str = "\u{e6a8}";
    pub const FAST_FORWARD: &str = "\u{e6a6}";
    pub const CARET_LEFT: &str = "\u{e138}";
    pub const CARET_RIGHT: &str = "\u{e13a}";
    pub const REPEAT: &str = "\u{e3f6}";
    pub const SPEAKER: &str = "\u{e44a}";
    pub const SPEAKER_OFF: &str = "\u{e45a}";
    pub const CAMERA: &str = "\u{e10e}";
    pub const FILM_SLATE: &str = "\u{e8c2}";
    pub const FOLDER_OPEN: &str = "\u{e256}";
    pub const WARNING: &str = "\u{e4e0}";
    pub const CHECK: &str = "\u{e184}";
    pub const X_CIRCLE: &str = "\u{e4f8}";
    pub const ZOOM_IN: &str = "\u{e310}";
    pub const ZOOM_OUT: &str = "\u{e30e}";
    pub const FIT: &str = "\u{e534}";
    pub const GEAR: &str = "\u{e270}";
    pub const QUESTION: &str = "\u{e3e8}";
    pub const QUEUE: &str = "\u{eadc}";
    pub const TERMINAL: &str = "\u{eae8}";
    pub const RETRY: &str = "\u{e036}";
    pub const TRASH: &str = "\u{e4a6}";
    pub const SLIDERS: &str = "\u{e434}";
    pub const EXPORT: &str = "\u{eaf0}";
    pub const KEYBOARD: &str = "\u{e2d8}";
    pub const PALETTE: &str = "\u{e6c8}";
    pub const MARK_IN: &str = "\u{e428}";
    pub const MARK_OUT: &str = "\u{e42a}";
    pub const CLOSE: &str = "\u{e4f6}";
    pub const FOLDER: &str = "\u{e24a}";
    pub const DOWNLOAD: &str = "\u{e20c}";
    pub const SUBTITLES: &str = "\u{e1a4}";
}
