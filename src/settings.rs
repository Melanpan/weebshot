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
            Self::Jpg => "JPEG",
            Self::Webp => "WebP",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SideTab {
    #[default]
    Capture,
    Stitch,
    Media,
    Queue,
    Logs,
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

    pub stitch: crate::stitch::Options,

    // Clip
    pub clip_format: ClipFormat,
    pub clip_stream_copy: bool,
    pub quality: Quality,
    pub use_target_size: bool,
    pub target_mb: f64,
    /// `None` = keep source resolution (video formats).
    pub max_height: Option<u32>,
    /// `None` = keep source frame rate (video formats).
    pub max_fps: Option<u32>,
    /// Height for GIF/WebP; these get huge at full resolution.
    pub anim_height: Option<u32>,
    /// Animated WebP compresses far better than GIF, so it gets its own, larger size.
    pub webp_height: Option<u32>,
    pub anim_fps: u32,
    pub anim: AnimOptions,
    pub include_audio: bool,
    pub clip_subs: SubMode,
    /// When the preview is zoomed in, export only the visible part.
    pub clip_crop: bool,
    pub encoder: EncoderOptions,
    /// Repeat the clip's content this many times in the output (1 = play once).
    pub loop_count: u32,

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
    pub side_tab: SideTab,
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
            shot_crop: false,
            stitch: crate::stitch::Options::default(),
            clip_format: ClipFormat::Webm,
            clip_stream_copy: false,
            quality: Quality::Balanced,
            use_target_size: false,
            target_mb: 8.0,
            max_height: None,
            max_fps: None,
            anim_height: Some(360),
            webp_height: Some(720),
            anim_fps: 15,
            anim: AnimOptions::default(),
            include_audio: true,
            clip_subs: SubMode::Burned,
            clip_crop: false,
            encoder: EncoderOptions::default(),
            loop_count: 1,
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
            side_tab: SideTab::Capture,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_copy_defaults_off_and_persists_when_enabled() {
        let mut settings: Settings = ron::from_str("(clip_format: Mp4)").unwrap();
        assert!(!settings.clip_stream_copy);
        settings.clip_stream_copy = true;
        let saved = ron::to_string(&settings).unwrap();
        let loaded: Settings = ron::from_str(&saved).unwrap();
        assert!(loaded.clip_stream_copy);
        assert_eq!(loaded.clip_format, ClipFormat::Mp4);
    }

    #[test]
    fn sidebar_defaults_to_capture_in_existing_settings() {
        let settings: Settings = ron::from_str("(shot_format: Jpg)").unwrap();
        assert_eq!(settings.side_tab, SideTab::Capture);
        assert_eq!(settings.shot_format, ShotFormat::Jpg);
    }

    #[test]
    fn selected_sidebar_tab_round_trips() {
        for side_tab in [
            SideTab::Capture,
            SideTab::Stitch,
            SideTab::Media,
            SideTab::Queue,
            SideTab::Logs,
        ] {
            let settings = Settings {
                side_tab,
                ..Default::default()
            };
            let saved = ron::to_string(&settings).unwrap();
            assert_eq!(
                ron::from_str::<Settings>(&saved).unwrap().side_tab,
                side_tab
            );
        }
    }
}
