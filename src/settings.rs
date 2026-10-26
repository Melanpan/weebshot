use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::export::{AnimOptions, ClipFormat, EncoderOptions, Quality, SubMode};
use crate::keymap::Keymap;
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShotFormat {
    Png,
    Jpg,
    Webp,
}

impl ShotFormat {
    pub const ALL: [Self; 3] = [Self::Png, Self::Jpg, Self::Webp];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Webp => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpg => "JPG",
            Self::Webp => "WebP",
        }
    }
}

pub const MAX_RECENT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepUnit {
    Frames,
    Seconds,
}

/// How far a step shortcut moves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub amount: f64,
    pub unit: StepUnit,
}

impl Step {
    pub const fn frames(amount: f64) -> Self {
        Self {
            amount,
            unit: StepUnit::Frames,
        }
    }

    pub const fn seconds(amount: f64) -> Self {
        Self {
            amount,
            unit: StepUnit::Seconds,
        }
    }
}

/// Everything the user can configure; persisted between runs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // Output
    /// `None` = save next to the source video.
    pub out_dir: Option<PathBuf>,
    pub shot_template: String,
    pub clip_template: String,

    // Screenshot
    pub shot_format: ShotFormat,
    pub shot_subs: SubMode,
    /// When the preview is zoomed in, save only the visible part.
    pub shot_crop: bool,

    // Clip
    pub clip_format: ClipFormat,
    pub quality: Quality,
    pub use_target_size: bool,
    pub target_mb: f64,
    /// `None` = keep source resolution (video formats).
    pub max_height: Option<u32>,
    /// Height for GIF/WebP; these get huge at full resolution.
    pub anim_height: Option<u32>,
    pub anim_fps: u32,
    pub anim: AnimOptions,
    pub include_audio: bool,
    pub clip_subs: SubMode,
    /// When the preview is zoomed in, export only the visible part.
    pub clip_crop: bool,
    pub encoder: EncoderOptions,

    // Playback
    pub volume: f64,
    pub audio_langs: String,
    pub sub_langs: String,
    pub hwdec: bool,
    /// Arrow keys / Shift+arrows / Ctrl+arrows.
    pub step_short: Step,
    pub step_medium: Step,
    pub step_long: Step,

    // Tools (None = auto-detect)
    pub ffmpeg_path: Option<PathBuf>,
    /// GPU encoders that passed detection.
    pub hw_available: Vec<crate::export::HwEncoder>,
    pub gifsicle_path: Option<PathBuf>,

    // Appearance / layout
    pub theme: Theme,
    pub ui_scale: f32,
    pub show_inspector: bool,
    pub show_sub_lane: bool,
    pub show_audio_lane: bool,
    /// Subtitles visible in the preview (exports are unaffected).
    pub show_subs_preview: bool,
    pub sounds: bool,
    pub sound_volume: f64,

    pub keymap: Keymap,
    pub recent: Vec<PathBuf>,

    // Updates
    pub check_updates: bool,
    /// Unix time of the last automatic check.
    pub last_update_check: u64,
    /// A version the user chose to skip.
    pub skipped_version: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            out_dir: None,
            shot_template: "{name}_{time}".into(),
            clip_template: "{name}_{start}-{end}".into(),
            shot_format: ShotFormat::Png,
            shot_subs: SubMode::Burned,
            shot_crop: true,
            clip_format: ClipFormat::Webm,
            quality: Quality::Balanced,
            use_target_size: false,
            target_mb: 8.0,
            max_height: None,
            anim_height: Some(360),
            anim_fps: 15,
            anim: AnimOptions::default(),
            include_audio: true,
            clip_subs: SubMode::Burned,
            clip_crop: true,
            encoder: EncoderOptions::default(),
            volume: 70.0,
            audio_langs: "jpn,ja,jp".into(),
            sub_langs: "eng,en,enm".into(),
            hwdec: true,
            step_short: Step::frames(1.0),
            step_medium: Step::seconds(1.0),
            step_long: Step::seconds(5.0),
            ffmpeg_path: None,
            hw_available: Vec::new(),
            gifsicle_path: None,
            theme: Theme::Studio,
            ui_scale: 1.0,
            show_inspector: true,
            show_sub_lane: true,
            show_audio_lane: true,
            show_subs_preview: true,
            sounds: true,
            sound_volume: 60.0,
            keymap: Keymap::default(),
            recent: Vec::new(),
            check_updates: true,
            last_update_check: 0,
            skipped_version: None,
        }
    }
}

impl Settings {
    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }
}
