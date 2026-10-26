//! Rebindable keyboard shortcuts.

use std::collections::BTreeMap;
use std::fmt;

use eframe::egui::{self, Key};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Action {
    Open,
    Close,
    Quit,
    Options,
    Docs,
    PlayPause,
    FrameBack,
    FrameForward,
    SeekBackSmall,
    SeekForwardSmall,
    SeekBackBig,
    SeekForwardBig,
    PrevSubtitle,
    NextSubtitle,
    ShuttleBack,
    ShuttleStop,
    ShuttleForward,
    ToggleSubtitles,
    SetIn,
    SetOut,
    MarkSubtitleLine,
    ClearInOut,
    GoToIn,
    GoToOut,
    ToggleLoop,
    Screenshot,
    ScreenshotClean,
    ExportClip,
    ToggleDock,
    Fullscreen,
    ZoomIn,
    ZoomOut,
    ZoomFit,
}

impl Action {
    pub const ALL: [Self; 33] = [
        Self::Open,
        Self::Close,
        Self::Quit,
        Self::Options,
        Self::Docs,
        Self::PlayPause,
        Self::FrameBack,
        Self::FrameForward,
        Self::SeekBackSmall,
        Self::SeekForwardSmall,
        Self::SeekBackBig,
        Self::SeekForwardBig,
        Self::PrevSubtitle,
        Self::NextSubtitle,
        Self::ShuttleBack,
        Self::ShuttleStop,
        Self::ShuttleForward,
        Self::ToggleSubtitles,
        Self::SetIn,
        Self::SetOut,
        Self::MarkSubtitleLine,
        Self::ClearInOut,
        Self::GoToIn,
        Self::GoToOut,
        Self::ToggleLoop,
        Self::Screenshot,
        Self::ScreenshotClean,
        Self::ExportClip,
        Self::ToggleDock,
        Self::Fullscreen,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomFit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open video",
            Self::Close => "Close video",
            Self::Quit => "Quit",
            Self::Options => "Options",
            Self::Docs => "Documentation",
            Self::PlayPause => "Play / pause",
            Self::FrameBack => "Step back",
            Self::FrameForward => "Step forward",
            Self::SeekBackSmall => "Step back (medium)",
            Self::SeekForwardSmall => "Step forward (medium)",
            Self::SeekBackBig => "Step back (long)",
            Self::SeekForwardBig => "Step forward (long)",
            Self::PrevSubtitle => "Previous subtitle line",
            Self::NextSubtitle => "Next subtitle line",
            Self::ShuttleBack => "Shuttle backwards (press again: faster)",
            Self::ShuttleStop => "Shuttle stop",
            Self::ShuttleForward => "Shuttle forwards (press again: faster)",
            Self::ToggleSubtitles => "Show/hide subtitles in the preview",
            Self::SetIn => "Set In point",
            Self::SetOut => "Set Out point",
            Self::MarkSubtitleLine => "Mark current subtitle line as In/Out",
            Self::ClearInOut => "Clear In/Out",
            Self::GoToIn => "Go to In point",
            Self::GoToOut => "Go to Out point",
            Self::ToggleLoop => "Loop In→Out",
            Self::Screenshot => "Screenshot",
            Self::ScreenshotClean => "Screenshot without subtitles",
            Self::ExportClip => "Export clip",
            Self::ToggleDock => "Switch Inspector / Queue",
            Self::Fullscreen => "Fullscreen",
            Self::ZoomIn => "Timeline zoom in",
            Self::ZoomOut => "Timeline zoom out",
            Self::ZoomFit => "Timeline zoom to fit",
        }
    }

    pub fn group(self) -> &'static str {
        use Action::*;
        match self {
            Open | Close | Quit | Options | Docs | ToggleDock | Fullscreen => "General",
            PlayPause | FrameBack | FrameForward | SeekBackSmall | SeekForwardSmall
            | SeekBackBig | SeekForwardBig | PrevSubtitle | NextSubtitle | ShuttleBack
            | ShuttleStop | ShuttleForward | ToggleSubtitles => "Playback",
            SetIn | SetOut | MarkSubtitleLine | ClearInOut | GoToIn | GoToOut | ToggleLoop => {
                "Marking"
            }
            Screenshot | ScreenshotClean | ExportClip => "Export",
            ZoomIn | ZoomOut | ZoomFit => "Timeline",
        }
    }

    /// Actions that should fire repeatedly while the key is held.
    pub fn repeats(self) -> bool {
        use Action::*;
        matches!(
            self,
            FrameBack
                | FrameForward
                | SeekBackSmall
                | SeekForwardSmall
                | SeekBackBig
                | SeekForwardBig
                | PrevSubtitle
                | NextSubtitle
                | ZoomIn
                | ZoomOut
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub key: Key,
    /// Ctrl on Windows/Linux, Cmd on macOS.
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Binding {
    pub const fn key(key: Key) -> Self {
        Self {
            key,
            command: false,
            shift: false,
            alt: false,
        }
    }
    pub const fn ctrl(key: Key) -> Self {
        Self {
            command: true,
            ..Self::key(key)
        }
    }
    pub const fn shift(key: Key) -> Self {
        Self {
            shift: true,
            ..Self::key(key)
        }
    }
    pub const fn alt(key: Key) -> Self {
        Self {
            alt: true,
            ..Self::key(key)
        }
    }

    pub fn from_event(key: Key, m: egui::Modifiers) -> Self {
        Self {
            key,
            command: m.command || m.ctrl,
            shift: m.shift,
            alt: m.alt,
        }
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.command {
            f.write_str(if cfg!(target_os = "macos") {
                "Cmd+"
            } else {
                "Ctrl+"
            })?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        f.write_str(key_text(self.key))
    }
}

/// Key name as shown in menus and docs.
pub fn key_text(key: Key) -> &'static str {
    match key {
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::Minus => "-",
        other => other.symbol_or_name(),
    }
}

pub fn default_binding(action: Action) -> Option<Binding> {
    use Action::*;
    Some(match action {
        Open => Binding::ctrl(Key::O),
        Close => Binding::ctrl(Key::W),
        Quit => Binding::ctrl(Key::Q),
        Options => Binding::ctrl(Key::Comma),
        Docs => Binding::key(Key::F1),
        PlayPause => Binding::key(Key::Space),
        FrameBack => Binding::key(Key::ArrowLeft),
        FrameForward => Binding::key(Key::ArrowRight),
        SeekBackSmall => Binding::shift(Key::ArrowLeft),
        SeekForwardSmall => Binding::shift(Key::ArrowRight),
        SeekBackBig => Binding::ctrl(Key::ArrowLeft),
        SeekForwardBig => Binding::ctrl(Key::ArrowRight),
        PrevSubtitle => Binding::key(Key::ArrowUp),
        NextSubtitle => Binding::key(Key::ArrowDown),
        ShuttleBack => Binding::key(Key::J),
        ShuttleStop => Binding::key(Key::K),
        ShuttleForward => Binding::key(Key::L),
        ToggleSubtitles => Binding::key(Key::C),
        SetIn => Binding::key(Key::I),
        SetOut => Binding::key(Key::O),
        MarkSubtitleLine => Binding::key(Key::X),
        ClearInOut => Binding::alt(Key::X),
        GoToIn => Binding::shift(Key::I),
        GoToOut => Binding::shift(Key::O),
        ToggleLoop => Binding::key(Key::P),
        Screenshot => Binding::key(Key::S),
        ScreenshotClean => Binding::ctrl(Key::S),
        ExportClip => Binding::key(Key::E),
        ToggleDock => Binding::ctrl(Key::J),
        Fullscreen => Binding::key(Key::F11),
        ZoomIn => Binding::key(Key::Equals),
        ZoomOut => Binding::key(Key::Minus),
        ZoomFit => Binding::shift(Key::Z),
    })
}

/// Bumped when default bindings change in a way saved keymaps should pick up.
const KEYMAP_VERSION: u32 = 4;

/// Action → binding. `None` means deliberately unbound.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymap {
    /// Missing in keymaps saved before versioning existed. The field-level default
    /// matters: the struct-level one would fill in the *current* version and skip migration.
    #[serde(default)]
    version: u32,
    bindings: BTreeMap<Action, Option<Binding>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            version: KEYMAP_VERSION,
            bindings: Action::ALL
                .iter()
                .map(|&a| (a, default_binding(a)))
                .collect(),
        }
    }
}

impl Keymap {
    /// Bring a saved keymap up to date: fill in new actions and move old defaults
    /// that changed (only where the user hadn't customized them).
    pub fn migrate(&mut self) {
        // v2 moved the long steps from J/L to Ctrl+arrows, but that migration never ran
        // (see `version`), so v4 repeats it for everyone.
        if self.version < 4 {
            for (action, old) in [
                (Action::SeekBackBig, Binding::key(Key::J)),
                (Action::SeekForwardBig, Binding::key(Key::L)),
            ] {
                if self.bindings.get(&action) == Some(&Some(old)) {
                    self.set(action, default_binding(action));
                }
            }
        }
        if self.version < 4 {
            // The shuttle keys may have been left unbound because J/L were still taken by
            // the long steps when they were added. Assign them now that they're free.
            for action in [
                Action::ShuttleBack,
                Action::ShuttleStop,
                Action::ShuttleForward,
                Action::ToggleSubtitles,
            ] {
                let default = default_binding(action);
                let free = default.is_some_and(|d| !self.bindings.values().any(|b| *b == Some(d)));
                if self.bindings.get(&action).is_none_or(|b| b.is_none()) && free {
                    self.bindings.insert(action, default);
                }
            }
        }
        self.version = KEYMAP_VERSION;
        // New actions get their default key, unless the user already uses it for something.
        for a in Action::ALL {
            if self.bindings.contains_key(&a) {
                continue;
            }
            let default = default_binding(a);
            let taken = default.is_some_and(|d| self.bindings.values().any(|b| *b == Some(d)));
            self.bindings.insert(a, if taken { None } else { default });
        }
    }

    pub fn get(&self, action: Action) -> Option<Binding> {
        self.bindings.get(&action).copied().flatten()
    }

    /// Assign `binding` to `action`, unbinding any other action that used it.
    /// Returns the action that lost its binding, if any.
    pub fn set(&mut self, action: Action, binding: Option<Binding>) -> Option<Action> {
        let mut displaced = None;
        if let Some(b) = binding {
            for (&other, slot) in self.bindings.iter_mut() {
                if other != action && *slot == Some(b) {
                    *slot = None;
                    displaced = Some(other);
                }
            }
        }
        self.bindings.insert(action, binding);
        displaced
    }

    pub fn text(&self, action: Action) -> String {
        self.get(action).map(|b| b.to_string()).unwrap_or_default()
    }

    /// Actions triggered by this frame's key presses.
    pub fn triggered(&self, ctx: &egui::Context) -> Vec<Action> {
        ctx.input(|i| {
            let mut out = Vec::new();
            for ev in &i.events {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = ev
                else {
                    continue;
                };
                let pressed = Binding::from_event(*key, *modifiers);
                for (&action, binding) in &self.bindings {
                    if *binding == Some(pressed) && (!repeat || action.repeats()) {
                        out.push(action);
                    }
                }
            }
            out
        })
    }

    /// Markdown table of all shortcuts, for the docs.
    pub fn markdown(&self) -> String {
        let mut s = String::new();
        let mut group = "";
        for a in Action::ALL {
            if a.group() != group {
                group = a.group();
                s += &format!("\n### {group}\n\n| Action | Shortcut |\n| --- | --- |\n");
            }
            let key = self
                .get(a)
                .map_or("*unbound*".to_string(), |b| format!("`{b}`"));
            s += &format!("| {} | {key} |\n", a.label());
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_actions_listed_once() {
        let mut seen = std::collections::BTreeSet::new();
        for a in Action::ALL {
            assert!(seen.insert(a), "{a:?} listed twice");
        }
    }

    #[test]
    fn defaults_have_no_conflicts() {
        let km = Keymap::default();
        let mut seen = std::collections::HashMap::new();
        for a in Action::ALL {
            if let Some(b) = km.get(a) {
                assert!(seen.insert(b.to_string(), a).is_none(), "{b} bound twice");
            }
        }
    }

    #[test]
    fn rebinding_steals_from_other_action() {
        let mut km = Keymap::default();
        let displaced = km.set(Action::Screenshot, Some(Binding::key(Key::I)));
        assert_eq!(displaced, Some(Action::SetIn));
        assert_eq!(km.get(Action::SetIn), None);
        assert_eq!(km.get(Action::Screenshot), Some(Binding::key(Key::I)));
    }

    #[test]
    fn migrates_old_long_step_keys() {
        let mut km = Keymap {
            version: 0,
            ..Default::default()
        };
        km.bindings
            .insert(Action::SeekBackBig, Some(Binding::key(Key::J)));
        km.bindings
            .insert(Action::SeekForwardBig, Some(Binding::key(Key::K)));
        km.bindings.remove(&Action::ZoomFit);
        km.migrate();
        assert_eq!(
            km.get(Action::SeekBackBig),
            Some(Binding::ctrl(Key::ArrowLeft))
        );
        // customized binding is kept
        assert_eq!(km.get(Action::SeekForwardBig), Some(Binding::key(Key::K)));
        assert_eq!(km.get(Action::ZoomFit), default_binding(Action::ZoomFit));
    }

    #[test]
    fn new_actions_dont_steal_keys() {
        let mut km = Keymap::default();
        km.bindings.remove(&Action::ShuttleStop);
        km.set(Action::Screenshot, Some(Binding::key(Key::K)));
        km.migrate();
        assert_eq!(km.get(Action::ShuttleStop), None);
        assert_eq!(km.get(Action::Screenshot), Some(Binding::key(Key::K)));
    }

    /// A keymap saved by an old build (no `version`, long steps on J/L, no shuttle keys).
    #[test]
    fn migrates_keymap_saved_without_version() {
        let saved = r#"(bindings: {
            SeekBackBig: Some((key: J, command: false, shift: false, alt: false)),
            SeekForwardBig: Some((key: L, command: false, shift: false, alt: false)),
        })"#;
        let mut km: Keymap = ron::from_str(saved).unwrap();
        assert_eq!(km.version, 0);
        km.migrate();
        assert_eq!(
            km.get(Action::SeekBackBig),
            Some(Binding::ctrl(Key::ArrowLeft))
        );
        assert_eq!(km.get(Action::ShuttleBack), Some(Binding::key(Key::J)));
        assert_eq!(km.get(Action::ShuttleForward), Some(Binding::key(Key::L)));
    }

    /// What v3 builds saved: version 3, long steps still on J/L, shuttle keys unbound.
    #[test]
    fn repairs_v3_keymap() {
        let mut km = Keymap {
            version: 3,
            ..Default::default()
        };
        km.bindings
            .insert(Action::SeekBackBig, Some(Binding::key(Key::J)));
        km.bindings
            .insert(Action::SeekForwardBig, Some(Binding::key(Key::L)));
        km.bindings.insert(Action::ShuttleBack, None);
        km.bindings.insert(Action::ShuttleForward, None);
        km.bindings
            .insert(Action::ToggleSubtitles, Some(Binding::key(Key::T)));
        km.migrate();
        assert_eq!(
            km.get(Action::SeekBackBig),
            Some(Binding::ctrl(Key::ArrowLeft))
        );
        assert_eq!(km.get(Action::ShuttleBack), Some(Binding::key(Key::J)));
        assert_eq!(km.get(Action::ShuttleForward), Some(Binding::key(Key::L)));
        // A binding the user chose is kept.
        assert_eq!(km.get(Action::ToggleSubtitles), Some(Binding::key(Key::T)));
    }

    #[test]
    fn binding_text() {
        assert_eq!(Binding::shift(Key::I).to_string(), "Shift+I");
    }
}
