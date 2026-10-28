//! The main window: state, actions and top-level layout.

mod dock;
mod docs;
mod inspector;
mod menu;
mod options;
pub mod shots;
mod timeline;
mod updates;
mod viewer;
mod widgets;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use crate::export::ffmpeg::StillJob;
use crate::export::{self, ClipFormat, ClipJob, Crop, JobEvent, Runner, SubMode, SubSource, Tools};
use crate::fonts::icon;
use crate::keymap::{Action, Binding};
use crate::log;
use crate::player::Player;
use crate::settings::Settings;
use crate::settings::{Step, StepUnit};
use crate::sounds::{Sound, Sounds};
use crate::subs::{CueLoader, CueSource};
use crate::theme::Palette;
use crate::thumbs::Thumbs;
use crate::tracks::{TrackKind, relative_sub_index};
use crate::util::{expand_template, frame_at, time_tag, unique_path};
use crate::waveform::Waveform;

pub use shots::Script;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "webm", "avi", "mov", "m2ts", "ts", "wmv", "flv", "m4v",
];

/// Tabs of the right-hand panel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SideTab {
    Inspector,
    Queue,
    Logs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OptionsPage {
    General,
    Playback,
    Export,
    Appearance,
    Shortcuts,
}

pub enum JobState {
    Queued,
    Running {
        stage: &'static str,
        /// Progress of the current step, `None` if it can't be measured.
        step: Option<f32>,
    },
    Done,
    Failed(String),
}

pub struct JobView {
    id: u64,
    name: String,
    job: ClipJob,
    progress: f32,
    state: JobState,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
}

#[derive(Default)]
struct LogView {
    min_level: Option<log::Level>,
    search: String,
    follow: bool,
}

/// A GPU encoder and whether its test encode worked.
type HwProbe = (export::HwEncoder, Result<(), String>);

/// Window/dialog state that isn't persisted.
struct UiState {
    show_options: bool,
    options_page: OptionsPage,
    show_docs: bool,
    docs_page: usize,
    docs_cache: egui_commonmark::CommonMarkCache,
    show_about: bool,
    show_update: bool,
    /// Action waiting for a key press in Options → Shortcuts.
    capturing: Option<Action>,
    side_tab: SideTab,
    /// Where the side panel was drawn last frame (for cropping docs screenshots).
    side_rect: egui::Rect,
    /// Running GPU encoder detection, and its last results.
    hw_probe: Option<crossbeam_channel::Receiver<Vec<HwProbe>>>,
    hw_results: Vec<HwProbe>,
    /// Force the GIF optimization section open (docs screenshots).
    gif_section_open: Option<bool>,
    log_view: LogView,
    /// Text being typed into the timecode field.
    timecode_edit: Option<String>,
}

pub struct ToolStatus {
    ffmpeg: Result<String, String>,
    gifsicle: Option<String>,
}

pub struct App {
    player: Player,
    settings: Settings,
    palette: Palette,
    tools: Tools,
    tool_status: ToolStatus,
    runner: Runner,
    jobs: Vec<JobView>,
    next_job_id: u64,
    in_point: Option<f64>,
    out_point: Option<f64>,
    looping: bool,
    timeline: timeline::TimelineState,
    cues: Option<CueLoader>,
    thumbs: Option<Thumbs>,
    waveform: Option<Waveform>,
    shuttle_ui: widgets::ShuttleState,
    /// Current shuttle speed (negative = backwards, 0 = off).
    shuttle: f64,
    /// Virtual playhead while shuttling backwards (done with seeks).
    reverse_pos: f64,
    /// Playing a single subtitle line: stop at this time (set at the given instant).
    play_until: Option<(f64, Instant)>,
    /// Automated run that captures the documentation screenshots.
    script: Option<shots::Script>,
    /// Where the timeline was drawn last frame (for cropping docs screenshots).
    timeline_rect: egui::Rect,
    ui: UiState,
    sounds: Sounds,
    /// A cropped screenshot being rendered by ffmpeg.
    shot_rx: Option<crossbeam_channel::Receiver<Result<Vec<String>, String>>>,
    /// Title last sent to the window, so it's only updated on change.
    window_title: String,
    updater: crate::update::Updater,
    view: viewer::ViewState,
    /// Actions requested by menus/buttons this frame, run after drawing.
    pending: Vec<Action>,
    message: Option<(String, bool, Instant)>,
}

fn detect_tools(settings: &Settings) -> (Tools, ToolStatus) {
    let ffmpeg = settings
        .ffmpeg_path
        .clone()
        .unwrap_or_else(|| export::find_tool("ffmpeg"));
    let gifsicle = settings
        .gifsicle_path
        .clone()
        .unwrap_or_else(|| export::find_tool("gifsicle"));
    let ffmpeg_status = export::tool_version(&ffmpeg, "-version").map_err(|e| format!("{e:#}"));
    let gifsicle_status = export::tool_version(&gifsicle, "--version").ok();
    match &ffmpeg_status {
        Ok(v) => log::info("app", format!("using {} ({v})", ffmpeg.display())),
        Err(e) => log::error("app", format!("ffmpeg unavailable: {e}")),
    }
    match &gifsicle_status {
        Some(v) => log::info("app", format!("using {} ({v})", gifsicle.display())),
        None => log::info("app", "gifsicle not found; GIF post-optimization disabled"),
    }
    (
        Tools {
            ffmpeg,
            gifsicle: gifsicle_status.is_some().then_some(gifsicle),
        },
        ToolStatus {
            ffmpeg: ffmpeg_status,
            gifsicle: gifsicle_status,
        },
    )
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_file: Option<PathBuf>,
        script: Option<shots::Script>,
    ) -> anyhow::Result<Self> {
        // The screenshot run uses default settings and never saves them.
        let mut settings: Settings = if script.is_some() {
            Settings::default()
        } else {
            cc.storage
                .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
                .unwrap_or_default()
        };
        settings.keymap.migrate();

        crate::fonts::install(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        // The screenshot run draws Options/Help inside the main window so one capture
        // shows them (separate native windows can't be captured from here).
        if script.is_some() {
            cc.egui_ctx.set_embed_viewports(true);
        }
        egui_extras::install_image_loaders(&cc.egui_ctx);
        docs::register_images(&cc.egui_ctx);
        settings.theme.apply(&cc.egui_ctx);
        cc.egui_ctx.set_zoom_factor(settings.ui_scale);

        let player = Player::new(cc, &settings)?;
        player.set_volume(settings.volume);
        player.set_sub_visibility(settings.show_subs_preview);

        let (tools, tool_status) = detect_tools(&settings);
        let runner = Runner::new(tools.clone(), cc.egui_ctx.clone());

        let mut app = Self {
            palette: settings.theme.palette(),
            player,
            settings,
            tools,
            tool_status,
            runner,
            jobs: Vec::new(),
            next_job_id: 1,
            in_point: None,
            out_point: None,
            looping: false,
            timeline: timeline::TimelineState::default(),
            cues: None,
            thumbs: None,
            waveform: None,
            shuttle_ui: Default::default(),
            shuttle: 0.0,
            reverse_pos: 0.0,
            play_until: None,
            script,
            timeline_rect: egui::Rect::NOTHING,
            ui: UiState {
                show_options: false,
                options_page: OptionsPage::General,
                show_docs: false,
                docs_page: 0,
                docs_cache: Default::default(),
                show_about: false,
                show_update: false,
                capturing: None,
                side_tab: SideTab::Inspector,
                side_rect: egui::Rect::NOTHING,
                gif_section_open: None,
                hw_probe: None,
                hw_results: Vec::new(),
                log_view: LogView {
                    follow: true,
                    ..Default::default()
                },
                timecode_edit: None,
            },
            sounds: Sounds::new(),
            shot_rx: None,
            window_title: String::new(),
            updater: crate::update::Updater::new(),
            view: Default::default(),
            pending: Vec::new(),
            message: None,
        };
        if let Some(path) = initial_file {
            app.open(&path);
        }
        app.maybe_check_for_updates(&cc.egui_ctx);
        Ok(app)
    }

    // ---- messages -------------------------------------------------------

    fn info(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info("app", msg.clone());
        self.message = Some((msg, false, Instant::now()));
    }

    fn error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::error("app", msg.clone());
        self.message = Some((msg, true, Instant::now()));
    }

    // ---- files ----------------------------------------------------------

    fn open(&mut self, path: &Path) {
        match self.player.load(path) {
            Ok(()) => {
                self.in_point = None;
                self.out_point = None;
                self.set_looping(false);
                self.cues = None;
                self.thumbs = None;
                self.waveform = None;
                self.timeline = timeline::TimelineState::default();
                self.settings.push_recent(path.to_path_buf());
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn open_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("Video", VIDEO_EXTENSIONS);
        if let Some(dir) = self.player.file.as_ref().and_then(|f| f.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open(&path);
        }
    }

    fn close(&mut self) {
        self.player.close();
        self.in_point = None;
        self.out_point = None;
        self.looping = false;
        self.cues = None;
    }

    fn out_dir(&self) -> Option<PathBuf> {
        self.settings
            .out_dir
            .clone()
            .or_else(|| self.player.file.as_ref()?.parent().map(Path::to_path_buf))
    }

    fn stem(&self) -> String {
        self.player
            .file
            .as_ref()
            .and_then(|f| f.file_stem())
            .map_or_else(|| "clip".into(), |s| s.to_string_lossy().into_owned())
    }

    // ---- marks ----------------------------------------------------------

    fn has_range(&self) -> bool {
        self.in_point.is_some() || self.out_point.is_some()
    }

    fn clip_range(&self) -> (f64, f64) {
        let start = self.in_point.unwrap_or(0.0);
        let end = self.out_point.unwrap_or(self.player.duration);
        (start, end)
    }

    fn set_looping(&mut self, on: bool) {
        self.looping = on;
        let (a, b) = self.clip_range();
        self.player
            .set_ab_loop((on && self.has_range() && b > a).then_some((a, b)));
    }

    fn set_in_at(&mut self, t: f64) {
        self.in_point = Some(t);
        if self.out_point.is_some_and(|o| o <= t) {
            self.out_point = None;
        }
        self.set_looping(self.looping);
    }

    fn set_out_at(&mut self, t: f64) {
        self.out_point = Some(t);
        if self.in_point.is_some_and(|i| i >= t) {
            self.in_point = None;
        }
        self.set_looping(self.looping);
    }

    fn set_range(&mut self, a: f64, b: f64) {
        self.in_point = Some(a);
        self.out_point = Some(b);
        self.set_looping(self.looping);
    }

    fn mark_subtitle_line(&mut self) {
        let line = self.player.current_sub_line().or_else(|| {
            let t = self.player.time;
            let cue = self.cues.as_ref()?.at(t)?;
            Some((cue.start, cue.end))
        });
        match line {
            Some((a, b)) => {
                self.set_range(a, b);
                self.info(format!("Marked subtitle line ({:.2}s)", b - a));
            }
            None => self.error("No subtitle line on screen"),
        }
    }

    /// Delete the cue selected by clicking it in the timeline (`Action::DeleteSubtitleLine`,
    /// `Delete` by default) — the keyboard-driven counterpart to the timeline's right-click
    /// "Delete subtitle". Deliberately not playhead-based: that would delete whatever line
    /// happened to be on screen, which can change out from under you while playing.
    fn delete_current_subtitle_line(&mut self) {
        match self.timeline.selected_cue {
            Some(idx) => {
                self.delete_cue(idx);
                self.info("Deleted subtitle line");
            }
            None => self.error("Click a subtitle line first to select it"),
        }
    }

    fn delete_cue(&mut self, idx: usize) {
        if let Some(c) = &mut self.cues {
            c.delete(idx);
        }
        // Indices after the deleted one shift down; a selection on the deleted line itself
        // no longer refers to anything.
        self.timeline.selected_cue = match self.timeline.selected_cue {
            Some(i) if i == idx => None,
            Some(i) if i > idx => Some(i - 1),
            other => other,
        };
        self.sync_preview_subs();
    }

    fn set_cue_text(&mut self, idx: usize, text: String) {
        if let Some(c) = &mut self.cues {
            c.set_text(idx, text);
        }
        self.sync_preview_subs();
    }

    fn retime_cue(&mut self, idx: usize, a: f64, b: f64) {
        if let Some(c) = &mut self.cues {
            c.retime(idx, a, b);
        }
        self.sync_preview_subs();
    }

    /// Once cues are edited, mpv shows them from a scratch .ass (reusing the original
    /// styles) instead of natively rendering the original track — there's no way to
    /// patch just one line of that in place. Rewritten and reloaded after every further
    /// edit; reverted once the edits no longer apply (track switched, or every line
    /// deleted).
    fn sync_preview_subs(&mut self) {
        match &self.cues {
            Some(loader) if loader.dirty && !loader.cues.is_empty() => {
                let path = std::env::temp_dir()
                    .join(format!("weebshot-{}-preview.subs.ass", std::process::id()));
                if loader.write_to(&path).is_ok() {
                    self.player.show_edited_subs(&path);
                }
            }
            _ => self.player.hide_edited_subs(),
        }
    }

    /// Subtitle mode that actually applies: forced off when no subtitle track is active.
    fn effective_sub_mode(&self, mode: SubMode) -> SubMode {
        if self.player.sid.is_some() {
            mode
        } else {
            SubMode::Off
        }
    }

    // ---- subtitle cues for the timeline ---------------------------------

    /// The cue source matching the current subtitle track, if it's text-based.
    fn cue_source(&self) -> Option<CueSource> {
        let t = self.player.current(TrackKind::Sub)?;
        if t.is_bitmap_sub() {
            return None;
        }
        Some(match &t.external {
            Some(p) => CueSource {
                file: p.clone(),
                ff_index: None,
            },
            None => CueSource {
                file: self.player.file.clone()?,
                ff_index: t.ff_index,
            },
        })
    }

    fn update_cues(&mut self, ctx: &egui::Context) {
        if let Some(c) = &mut self.cues {
            c.poll();
        }
        if !self.settings.show_sub_lane || self.tool_status.ffmpeg.is_err() {
            return;
        }
        let wanted = self.cue_source();
        match (wanted, &self.cues) {
            (Some(w), Some(c)) if c.source == w => return,
            (Some(w), _) => {
                self.cues = Some(CueLoader::spawn(self.tools.ffmpeg.clone(), w, ctx.clone()));
            }
            (None, Some(_)) => self.cues = None,
            (None, None) => return,
        }
        // The track changed: any previous edits/selection no longer apply.
        self.timeline.selected_cue = None;
        self.sync_preview_subs();
    }

    /// Subtitle cues are read first so the subtitle lane shows up quickly; thumbnails and the
    /// waveform wait for them instead of competing for the disk.
    fn cues_loading(&self) -> bool {
        self.cues.as_ref().is_some_and(|c| c.loading())
    }

    // ---- export ---------------------------------------------------------

    fn template_vars(&self, extra: &[(&str, String)]) -> Vec<(&'static str, String)> {
        let t = self.player.time;
        let (a, b) = self.clip_range();
        let mut vars = vec![
            ("name", self.stem()),
            ("time", time_tag(t)),
            ("frame", frame_at(t, self.player.fps).to_string()),
            ("start", time_tag(a)),
            ("end", time_tag(b)),
        ];
        for (k, v) in extra {
            if let Some(slot) = vars.iter_mut().find(|(n, _)| n == k) {
                slot.1 = v.clone();
            }
        }
        vars
    }

    fn take_screenshot(&mut self, subs: SubMode) {
        let Some(dir) = self.out_dir() else {
            return self.error("Open a video first");
        };
        let format = self.settings.shot_format;
        let base = expand_template(&self.settings.shot_template, &self.template_vars(&[]));
        let variants: &[(bool, &str)] = match self.effective_sub_mode(subs) {
            SubMode::Off => &[(false, "")],
            SubMode::Burned => &[(true, "")],
            SubMode::Both => &[(true, "_subbed"), (false, "_raw")],
        };
        if self.settings.shot_crop
            && let Some(crop) = self.view_crop()
        {
            return self.cropped_screenshot(crop, variants, &dir, &base);
        }
        let mut saved = Vec::new();
        for &(subs, suffix) in variants {
            let path = unique_path(&dir, &format!("{base}{suffix}"), format.ext());
            if let Err(e) = self.player.screenshot(&path, format, subs) {
                return self.error(format!("{e:#}"));
            }
            log::info("app", format!("screenshot {}", path.display()));
            saved.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
        self.play_sound(Sound::Screenshot);
        self.info(format!("Saved {}", saved.join(", ")));
    }

    /// Screenshot of just the zoomed-in part of the frame, rendered by ffmpeg (mpv's own
    /// screenshots are always the full frame). Runs in the background; `poll_screenshots`
    /// reports the result.
    fn cropped_screenshot(
        &mut self,
        crop: Crop,
        variants: &[(bool, &str)],
        dir: &Path,
        base: &str,
    ) {
        let (Some(input), Ok(_)) = (self.player.file.clone(), &self.tool_status.ffmpeg) else {
            return self.error("Cropped screenshots need ffmpeg");
        };
        let subs = match self.sub_source() {
            Ok(s) => s,
            Err(e) => return self.error(e),
        };
        let jobs: Vec<StillJob> = variants
            .iter()
            .map(|&(burn, suffix)| StillJob {
                input: input.clone(),
                time: self.player.time,
                fps: self.player.fps,
                crop: Some(crop),
                subs: if burn { subs.clone() } else { None },
                output: unique_path(
                    dir,
                    &format!("{base}{suffix}"),
                    self.settings.shot_format.ext(),
                ),
            })
            .collect();
        let ffmpeg = self.tools.ffmpeg.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let mut saved = Vec::new();
            for job in jobs {
                if let Err(e) = export::run_still(&ffmpeg, &job) {
                    let _ = tx.send(Err(e));
                    return;
                }
                saved.push(
                    job.output
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            let _ = tx.send(Ok(saved));
        });
        self.shot_rx = Some(rx);
        self.info(format!("Saving {}×{} crop…", crop.width, crop.height));
    }

    fn poll_screenshots(&mut self) {
        let Some(rx) = &self.shot_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.shot_rx = None;
        match result {
            Ok(saved) => {
                self.play_sound(Sound::Screenshot);
                self.info(format!("Saved {}", saved.join(", ")));
            }
            Err(e) => self.error(format!("Screenshot failed: {e}")),
        }
    }

    fn sub_source(&self) -> Result<Option<SubSource>, String> {
        let Some(track) = self.player.current(TrackKind::Sub) else {
            return Ok(None);
        };
        let input = self.player.file.clone().unwrap_or_default();
        if track.is_bitmap_sub() {
            if track.external.is_some() {
                return Err("Burning external image-based subtitles isn't supported".into());
            }
            let ff_index = track.ff_index.ok_or("Subtitle track has no stream index")?;
            return Ok(Some(SubSource::Bitmap { ff_index }));
        }
        // Edited cues (delete/retime/edit text) for the current track — burn those in
        // instead of the original file, by writing them to a scratch .ass that reuses
        // the original style definitions. Only kicks in once actually edited, so an
        // untouched track keeps its original styling exactly. Not cleaned up immediately:
        // a queued export job may still be reading it.
        if let Some(loader) = &self.cues
            && loader.dirty
            && !loader.loading()
            && !loader.cues.is_empty()
            && self.cue_source().is_some_and(|w| w == loader.source)
        {
            let path = std::env::temp_dir().join(format!(
                "weebshot-{}-{}.subs.ass",
                std::process::id(),
                self.next_job_id
            ));
            loader.write_to(&path).map_err(|e| e.to_string())?;
            return Ok(Some(SubSource::Text { path, stream: 0 }));
        }
        Ok(Some(match &track.external {
            Some(path) => SubSource::Text {
                path: path.clone(),
                stream: 0,
            },
            None => SubSource::Text {
                path: input,
                stream: relative_sub_index(&self.player.tracks, track),
            },
        }))
    }

    fn export_clip(&mut self) {
        let (Some(input), Some(dir)) = (self.player.file.clone(), self.out_dir()) else {
            return self.error("Open a video first");
        };
        if self.tool_status.ffmpeg.is_err() {
            return self.error("ffmpeg not found — see Options → Export");
        }
        if !self.has_range() {
            return self.error("Set an In and/or Out point first (I / O, or X on a subtitle line)");
        }
        let (start, end) = self.clip_range();
        if end - start < 0.05 {
            return self.error("The In→Out range is empty");
        }

        let s = &self.settings;
        let format = s.clip_format;
        let audio_ff_index = (s.include_audio && format.has_audio())
            .then(|| self.player.current(TrackKind::Audio)?.ff_index)
            .flatten();
        let base = ClipJob {
            input,
            start,
            end,
            format,
            quality: s.quality,
            target_mb: (s.use_target_size && format.supports_target_size()).then_some(s.target_mb),
            audio_ff_index,
            subs: None,
            crop: if s.clip_crop { self.view_crop() } else { None },
            max_height: match format {
                ClipFormat::Gif => s.anim_height,
                ClipFormat::Webp => s.webp_height,
                _ => s.max_height,
            },
            src_height: self.player.video_height,
            max_fps: s.max_fps,
            src_fps: Some(self.player.fps),
            fps: s.anim_fps,
            anim: s.anim,
            encoder: s.encoder.clone(),
            use_gifsicle: self.tools.gifsicle.is_some(),
            output: PathBuf::new(),
        };
        if let Err(e) = base.target_video_kbps() {
            return self.error(format!("{e:#}"));
        }

        let mode = self.effective_sub_mode(s.clip_subs);
        let subs = match mode {
            SubMode::Off => None,
            _ => match self.sub_source() {
                Ok(src) => src,
                Err(e) => return self.error(e),
            },
        };
        let variants: Vec<(Option<SubSource>, &str)> = match (mode, subs) {
            (SubMode::Burned, Some(src)) => vec![(Some(src), "")],
            (SubMode::Both, Some(src)) => vec![(Some(src), "_subbed"), (None, "_raw")],
            _ => vec![(None, "")],
        };

        let stem = expand_template(&self.settings.clip_template, &self.template_vars(&[]));
        for (subs, suffix) in variants {
            let output = unique_path(&dir, &format!("{stem}{suffix}"), format.ext());
            let job = ClipJob {
                subs,
                output,
                ..base.clone()
            };
            self.submit(job);
        }
        let n = self.jobs_active();
        self.info(format!("Export queued ({n} in queue) — see the Queue tab"));
    }

    fn export_subtitles(&mut self) {
        let Some(loader) = &self.cues else {
            return self.error("No subtitles loaded — show the Subtitles lane first");
        };
        if loader.cues.is_empty() {
            return self.error("No subtitle lines to save");
        }
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("subtitles.ass")
            .add_filter("Advanced SubStation Alpha", &["ass"])
            .save_file()
        else {
            return;
        };
        match loader.write_to(&path) {
            Ok(()) => self.info(format!("Saved {}", path.display())),
            Err(e) => self.error(format!("Couldn't save subtitles: {e}")),
        }
    }

    fn submit(&mut self, job: ClipJob) {
        let id = self.next_job_id;
        self.next_job_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let name = job
            .output
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        log::info("export", format!("queued {name}"));
        self.runner.submit(id, job.clone(), cancel.clone());
        self.jobs.push(JobView {
            id,
            name,
            job,
            progress: 0.0,
            state: JobState::Queued,
            cancel,
            started: None,
            elapsed: None,
        });
    }

    fn play_sound(&self, sound: Sound) {
        if self.settings.sounds && self.script.is_none() {
            self.sounds.play(sound, self.settings.sound_volume);
        }
    }

    fn poll_jobs(&mut self) {
        let mut failed = false;
        let mut finished = false;
        while let Ok(ev) = self.runner.events.try_recv() {
            let id = match &ev {
                JobEvent::Progress { id, .. } | JobEvent::Finished { id, .. } => *id,
            };
            let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else {
                continue;
            };
            match ev {
                JobEvent::Progress {
                    fraction,
                    overall,
                    stage,
                    ..
                } => {
                    job.started.get_or_insert_with(Instant::now);
                    job.progress = overall;
                    job.state = JobState::Running {
                        stage,
                        step: fraction,
                    };
                }
                JobEvent::Finished { result, .. } => {
                    job.elapsed = job.started.map(|s| s.elapsed());
                    finished = true;
                    job.state = match result {
                        Ok(()) => JobState::Done,
                        Err(e) => {
                            failed |= e != "cancelled";
                            JobState::Failed(e)
                        }
                    }
                }
            }
        }
        // One chime when the whole queue is through; failures sound right away.
        if failed {
            self.play_sound(Sound::ExportFailed);
        } else if finished && self.jobs_active() == 0 {
            self.play_sound(Sound::ExportDone);
        }
    }

    fn jobs_active(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Queued | JobState::Running { .. }))
            .count()
    }

    fn redetect_tools(&mut self, ctx: &egui::Context) {
        if self.jobs_active() > 0 {
            return self.error("Wait for running exports to finish first");
        }
        let (tools, status) = detect_tools(&self.settings);
        self.runner = Runner::new(tools.clone(), ctx.clone());
        self.tools = tools;
        self.tool_status = status;
    }

    // ---- actions --------------------------------------------------------

    /// Start/stop/change shuttle playback. Forwards uses mpv's speed; backwards is done by
    /// seeking (mpv's reverse playback is very slow), see `tick_playback`.
    fn set_shuttle(&mut self, speed: f64) {
        let was = self.shuttle;
        self.shuttle = speed;
        self.play_until = None;
        if speed > 0.0 {
            self.player.set_speed(speed);
            self.player.set_mute(speed > 2.0);
            self.player.set_pause(false);
        } else {
            self.player.set_speed(1.0);
            self.player.set_mute(false);
            self.player.set_pause(true);
            if speed < 0.0 && was >= 0.0 {
                self.reverse_pos = self.player.time;
            }
        }
    }

    /// Move the shuttle `steps` speed levels up (towards fast forward) or down.
    fn nudge_shuttle(&mut self, steps: i32) {
        let levels = widgets::SHUTTLE_STEPS;
        let current = levels
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1 - self.shuttle)
                    .abs()
                    .total_cmp(&(b.1 - self.shuttle).abs())
            })
            .map_or(levels.len() / 2, |(i, _)| i) as i32;
        let next = (current + steps).clamp(0, levels.len() as i32 - 1) as usize;
        self.set_shuttle(levels[next]);
    }

    fn stop_shuttle(&mut self) {
        if self.shuttle != 0.0 {
            self.set_shuttle(0.0);
        }
    }

    /// Play from `a` to `b` once, then pause.
    fn play_range(&mut self, a: f64, b: f64) {
        self.stop_shuttle();
        self.player.seek(a, true);
        self.player.set_pause(false);
        self.play_until = Some((b, Instant::now()));
    }

    /// Preview the whole marked clip: play from In through to Out (or the end, if no
    /// Out is set), then pause.
    fn play_from_in(&mut self) {
        let Some(a) = self.in_point else {
            return self.error("Set an In point first");
        };
        let b = self.out_point.unwrap_or(self.player.duration);
        self.play_range(a, b);
    }

    /// Preview where the Out point lands: play from the playhead to Out, then pause,
    /// without jumping back to In first.
    fn play_to_out(&mut self) {
        let Some(b) = self.out_point else {
            return self.error("Set an Out point first");
        };
        let a = self.player.time;
        if a >= b {
            return self.error("Already past the Out point");
        }
        self.play_range(a, b);
    }

    /// Per-frame playback bookkeeping: reverse shuttle and play-a-line.
    fn tick_playback(&mut self, ctx: &egui::Context) {
        if self.shuttle < 0.0 {
            let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
            self.reverse_pos = (self.reverse_pos + self.shuttle * dt).max(0.0);
            // Exact frames at slow speeds; nearest keyframes when going fast.
            self.player
                .seek(self.reverse_pos, self.shuttle.abs() <= 1.0);
            if self.reverse_pos <= 0.0 {
                self.set_shuttle(0.0);
            }
            ctx.request_repaint();
        }
        if let Some((end, started)) = self.play_until {
            let frame = 1.0 / self.player.fps;
            if self.player.time >= end - frame / 2.0 {
                self.player.set_pause(true);
                self.play_until = None;
            } else if self.player.paused && started.elapsed() > Duration::from_millis(500) {
                // Paused by the user.
                self.play_until = None;
            } else {
                ctx.request_repaint();
            }
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, action: Action) {
        use Action::*;
        // Any other transport action takes over from the shuttle / play-a-line.
        if matches!(
            action,
            PlayPause
                | FrameBack
                | FrameForward
                | SeekBackSmall
                | SeekForwardSmall
                | SeekBackBig
                | SeekForwardBig
                | PrevSubtitle
                | NextSubtitle
                | GoToIn
                | GoToOut
        ) {
            self.play_until = None;
            if self.shuttle != 0.0 {
                self.stop_shuttle();
                if action == PlayPause {
                    return; // stopping the shuttle is the pause
                }
            }
        }
        let s = &self.settings;
        let (short, medium, long) = (s.step_short, s.step_medium, s.step_long);
        match action {
            Action::Open => self.open_dialog(),
            Action::Close => self.close(),
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Options => self.ui.show_options = !self.ui.show_options,
            Action::Docs => self.ui.show_docs = !self.ui.show_docs,
            Action::PlayPause => self.player.toggle_pause(),
            Action::FrameBack => self.step(short, -1.0),
            Action::FrameForward => self.step(short, 1.0),
            Action::SeekBackSmall => self.step(medium, -1.0),
            Action::SeekForwardSmall => self.step(medium, 1.0),
            Action::SeekBackBig => self.step(long, -1.0),
            Action::SeekForwardBig => self.step(long, 1.0),
            Action::ShuttleForward => {
                let s = self.shuttle;
                self.set_shuttle(if s <= 0.0 {
                    1.0
                } else {
                    (s * 2.0).min(widgets::MAX_SHUTTLE_SPEED)
                });
            }
            Action::ShuttleBack => {
                let s = self.shuttle;
                self.set_shuttle(if s >= 0.0 {
                    -1.0
                } else {
                    (s * 2.0).max(-widgets::MAX_SHUTTLE_SPEED)
                });
            }
            Action::ShuttleStop => self.set_shuttle(0.0),
            Action::ToggleSubtitles => {
                self.settings.show_subs_preview = !self.settings.show_subs_preview;
                self.player
                    .set_sub_visibility(self.settings.show_subs_preview);
            }
            Action::PrevSubtitle => self.player.sub_seek(false),
            Action::NextSubtitle => self.player.sub_seek(true),
            Action::SetIn => self.set_in_at(self.player.time),
            Action::SetOut => self.set_out_at(self.player.time),
            Action::MarkSubtitleLine => self.mark_subtitle_line(),
            Action::DeleteSubtitleLine => self.delete_current_subtitle_line(),
            Action::ClearInOut => {
                self.in_point = None;
                self.out_point = None;
                self.set_looping(false);
            }
            Action::GoToIn => {
                if let Some(t) = self.in_point {
                    self.player.seek(t, true);
                }
            }
            Action::GoToOut => {
                if let Some(t) = self.out_point {
                    self.player.seek(t, true);
                }
            }
            Action::PlayFromIn => self.play_from_in(),
            Action::PlayToOut => self.play_to_out(),
            Action::ToggleLoop => self.set_looping(!self.looping),
            Action::Screenshot => self.take_screenshot(self.settings.shot_subs),
            Action::ScreenshotClean => self.take_screenshot(SubMode::Off),
            Action::Fullscreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            Action::ExportClip => self.export_clip(),
            Action::ExportSubtitles => self.export_subtitles(),
            Action::ToggleDock => {
                self.settings.show_inspector = true;
                self.ui.side_tab = if self.ui.side_tab == SideTab::Inspector {
                    SideTab::Queue
                } else {
                    SideTab::Inspector
                };
            }
            Action::ViewZoomIn => self.zoom_view_by(1.25),
            Action::ViewZoomOut => self.zoom_view_by(0.8),
            Action::ViewZoomFit => self.reset_view_zoom(),
            Action::ZoomIn => self.timeline.zoom(0.5, None, self.player.duration),
            Action::ZoomOut => self.timeline.zoom(2.0, None, self.player.duration),
            Action::ZoomFit => self.timeline.fit(),
        }
    }

    /// Move by a configured step; frame steps pause playback like an editor does.
    fn step(&mut self, step: Step, direction: f64) {
        match step.unit {
            StepUnit::Frames => {
                self.player.set_pause(true);
                self.player
                    .step_frames((step.amount.round().max(1.0) * direction) as i64);
            }
            StepUnit::Seconds => self.player.seek_relative(step.amount * direction),
        }
    }

    /// (Re)load the waveform when the file or the audio track changes.
    fn update_waveform(&mut self, ctx: &egui::Context) {
        if let Some(w) = &mut self.waveform {
            w.poll();
        }
        if !self.settings.show_audio_lane || self.tool_status.ffmpeg.is_err() || self.cues_loading()
        {
            return;
        }
        let wanted = self.player.file.clone().zip(
            self.player
                .current(TrackKind::Audio)
                .filter(|t| t.external.is_none())
                .and_then(|t| t.ff_index),
        );
        match (wanted, &self.waveform) {
            (Some((file, ff)), Some(w)) if w.matches(&file, ff) => {}
            (Some((file, ff)), _) => {
                self.waveform = Some(Waveform::spawn(
                    self.tools.ffmpeg.clone(),
                    file,
                    ff,
                    ctx.clone(),
                ));
            }
            (None, _) => self.waveform = None,
        }
    }

    fn update_thumbs(&mut self, ctx: &egui::Context) {
        if let Some(t) = &mut self.thumbs {
            t.poll();
        }
        let (Some(file), Some(w), Some(h)) = (
            &self.player.file,
            self.player.video_width,
            self.player.video_height,
        ) else {
            return;
        };
        if self.tool_status.ffmpeg.is_err()
            || self.cues_loading()
            || self.thumbs.as_ref().is_some_and(|t| t.matches(file))
        {
            return;
        }
        self.thumbs = Some(Thumbs::new(
            self.tools.ffmpeg.clone(),
            file.clone(),
            w,
            h,
            self.player.fps,
            ctx.clone(),
        ));
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open(&path);
        }

        // Rebinding a shortcut swallows the next key press.
        if let Some(action) = self.ui.capturing {
            let pressed = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some(Binding::from_event(*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some(b) = pressed {
                self.ui.capturing = None;
                if b.key != egui::Key::Escape
                    && let Some(other) = self.settings.keymap.set(action, Some(b))
                {
                    self.info(format!("{b} moved from \"{}\"", other.label()));
                }
            }
            return;
        }

        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let actions = self.settings.keymap.triggered(ctx);
        if !actions.is_empty() {
            // A focused button would also react to Space/Enter and undo the shortcut.
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        for action in actions {
            self.run_action(ctx, action);
        }
    }

    // ---- small shared widgets --------------------------------------------

    /// Button showing its shortcut; queues the action when clicked.
    fn action_button(&mut self, ui: &mut egui::Ui, action: Action, label: &str) -> egui::Response {
        let shortcut = self.settings.keymap.text(action);
        let tip = if shortcut.is_empty() {
            action.label().to_string()
        } else {
            format!("{} ({shortcut})", action.label())
        };
        let r = ui.button(label).on_hover_text(tip);
        if r.clicked() {
            self.pending.push(action);
        }
        r
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        ui.horizontal(|ui| {
            if let Some((msg, is_err, when)) = &self.message
                && when.elapsed() < Duration::from_secs(10)
            {
                let icon = if *is_err { icon::WARNING } else { icon::CHECK };
                ui.colored_label(
                    if *is_err { p.error } else { p.ok },
                    format!("{icon} {msg}"),
                );
            } else if let Some(f) = &self.player.file {
                ui.weak(f.display().to_string());
            } else {
                ui.weak("No video loaded");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Timeline zoom, like an editor's bottom-right zoom slider.
                let duration = self.player.duration;
                ui.add_enabled_ui(duration > 0.0, |ui| {
                    if ui
                        .small_button(icon::ZOOM_IN)
                        .on_hover_text("Zoom timeline in")
                        .clicked()
                    {
                        self.pending.push(Action::ZoomIn);
                    }
                    let mut level = self.timeline.zoom_level(duration);
                    let slider = egui::Slider::new(&mut level, 0.0..=1.0).show_value(false);
                    if ui
                        .add_sized([110.0, 16.0], slider)
                        .on_hover_text("Timeline zoom (Ctrl+wheel over the timeline)")
                        .changed()
                    {
                        let anchor = self.player.time;
                        self.timeline.set_zoom_level(level, anchor, duration);
                    }
                    if ui
                        .small_button(icon::ZOOM_OUT)
                        .on_hover_text("Zoom timeline out")
                        .clicked()
                    {
                        self.pending.push(Action::ZoomOut);
                    }
                    if ui
                        .small_button(icon::FIT)
                        .on_hover_text("Fit whole file")
                        .clicked()
                    {
                        self.pending.push(Action::ZoomFit);
                    }
                });
                ui.separator();
                let errors = log::unseen_errors();
                if errors > 0
                    && ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{} {errors}", icon::WARNING)).color(p.error),
                            )
                            .small(),
                        )
                        .on_hover_text("Errors logged — open the Logs tab")
                        .clicked()
                {
                    self.settings.show_inspector = true;
                    self.ui.side_tab = SideTab::Logs;
                }
                if let Some((progress, label)) = self.queue_progress() {
                    let r = ui
                        .add(egui::Label::new(label).sense(egui::Sense::click()))
                        .on_hover_text("Open the Queue");
                    let wheel = widgets::progress_ring(ui, progress, 16.0, p.accent, p.bg3);
                    if r.clicked() || wheel.clicked() {
                        self.settings.show_inspector = true;
                        self.ui.side_tab = SideTab::Queue;
                    }
                }
                if let crate::update::State::Available(r) = &self.updater.state
                    && self.settings.skipped_version.as_deref() != Some(r.version.as_str())
                {
                    let text = RichText::new(format!("{} Update to {}", icon::DOWNLOAD, r.version))
                        .color(p.accent);
                    if ui.add(egui::Button::new(text).small()).clicked() {
                        self.ui.show_update = true;
                    }
                    ui.separator();
                }
                let tasks = self.background_tasks();
                if !tasks.is_empty() {
                    ui.separator();
                    let label = tasks
                        .iter()
                        .map(|(name, progress)| match progress {
                            Some(f) => format!("{name} {:.0}%", f * 100.0),
                            None => format!("{name}…"),
                        })
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ui.weak(label);
                    // A real percentage only when it's the sole task.
                    let progress = match tasks.as_slice() {
                        [(_, p)] => *p,
                        _ => None,
                    };
                    widgets::progress_ring(ui, progress, 14.0, p.fg_dim, p.bg3);
                }
                if self.tool_status.ffmpeg.is_err() {
                    ui.colored_label(p.error, format!("{} ffmpeg not found", icon::WARNING));
                }
            });
        });
    }

    /// "episode.mkv — Weebshot" while a file is open.
    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.player.file.as_ref().and_then(|f| f.file_name()) {
            Some(name) => format!("{} — Weebshot", name.to_string_lossy()),
            None => "Weebshot".to_string(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    /// Background work for the open file: (label, progress if measurable).
    fn background_tasks(&self) -> Vec<(&'static str, Option<f32>)> {
        let mut tasks = Vec::new();
        if self.thumbs.as_ref().is_some_and(|t| t.busy()) {
            tasks.push(("Generating thumbnails", None));
        }
        if let Some(w) = self.waveform.as_ref().filter(|w| w.loading()) {
            tasks.push(("Reading audio", Some(w.progress(self.player.duration))));
        }
        if self.cues.as_ref().is_some_and(|c| c.loading()) {
            tasks.push(("Reading subtitles", None));
        }
        tasks
    }

    /// Overall progress of the running export (`None` inside = indeterminate) and a label;
    /// `None` when the queue is idle.
    fn queue_progress(&self) -> Option<(Option<f32>, String)> {
        let active = self.jobs_active();
        let running = self
            .jobs
            .iter()
            .find(|j| matches!(j.state, JobState::Running { .. }))?;
        let JobState::Running { stage, step } = &running.state else {
            return None;
        };
        let progress = step.map(|_| running.progress);
        let pct = progress.map_or(String::new(), |p| format!(" {:.0}%", p * 100.0));
        let more = if active > 1 {
            format!(" (+{} queued)", active - 1)
        } else {
            String::new()
        };
        Some((progress, format!("{stage}{pct}{more}")))
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        self.ui.side_rect = ui.max_rect().expand(8.0);
        let p = self.palette;
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.ui.side_tab,
                SideTab::Inspector,
                format!("{} Inspector", icon::SLIDERS),
            );
            let active = self.jobs_active();
            let queue = if active > 0 {
                format!("{} Queue ({active})", icon::QUEUE)
            } else {
                format!("{} Queue", icon::QUEUE)
            };
            ui.selectable_value(&mut self.ui.side_tab, SideTab::Queue, queue);
            let errors = log::unseen_errors();
            let logs = if errors > 0 {
                RichText::new(format!("{} Logs {errors}", icon::TERMINAL)).color(p.error)
            } else {
                RichText::new(format!("{} Logs", icon::TERMINAL))
            };
            ui.selectable_value(&mut self.ui.side_tab, SideTab::Logs, logs);
        });
        ui.separator();
        match self.ui.side_tab {
            SideTab::Inspector => {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.inspector(ui));
            }
            SideTab::Queue => self.queue_tab(ui),
            SideTab::Logs => {
                log::mark_seen();
                self.logs_tab(ui);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.player.poll();
        self.poll_jobs();
        self.update_cues(&ctx);
        self.update_thumbs(&ctx);
        self.update_waveform(&ctx);
        self.poll_updater(&ctx);
        self.poll_screenshots();
        self.update_window_title(&ctx);
        if self.script.is_none() {
            self.handle_input(&ctx);
        }
        self.tick_playback(&ctx);

        let panel_frame = egui::Frame::new()
            .fill(self.palette.bg1)
            .inner_margin(egui::Margin::symmetric(6, 3));

        egui::Panel::top("menu")
            .frame(panel_frame)
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status")
            .frame(panel_frame)
            .show(ui, |ui| self.status_bar(ui));
        let fitted =
            timeline::fitted_height(self.settings.show_audio_lane, self.settings.show_sub_lane);
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(fitted)
            .min_size(fitted.min(140.0))
            .frame(
                egui::Frame::new()
                    .fill(self.palette.bg1)
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show(ui, |ui| self.timeline_panel(ui));
        if self.settings.show_inspector {
            egui::Panel::right("side")
                .resizable(true)
                .default_size(440.0)
                .min_size(280.0)
                .frame(
                    egui::Frame::new()
                        .fill(self.palette.bg1)
                        .inner_margin(egui::Margin::same(8)),
                )
                .show(ui, |ui| self.side_panel(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.palette.bg0))
            .show(ui, |ui| self.viewer(ui));

        self.options_window(&ctx);
        self.docs_window(&ctx);
        self.about_window(&ctx);
        self.update_window(&ctx);

        for action in std::mem::take(&mut self.pending) {
            self.run_action(&ctx, action);
        }
        if self.script.is_some() {
            self.run_script(&ctx);
        }

        // Keep the clock ticking even if no new video frame arrives.
        if !self.player.paused || self.jobs_active() > 0 {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.script.is_some() {
            return;
        }
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    /// The docs screenshot run must not pick up (or overwrite) the user's panel sizes.
    fn persist_egui_memory(&self) -> bool {
        self.script.is_none()
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.player.destroy_gl(gl);
    }
}

/// The window icon (the mascot), decoded once.
pub fn app_icon() -> Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon.png"))
                .unwrap_or_default(),
        )
    })
    .clone()
}

/// Show `add` in its own native OS window (title bar, close button, resizing by the OS).
/// Returns `false` once the user closes it.
fn native_window(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: [f32; 2],
    min_size: [f32; 2],
    mut add: impl FnMut(&mut egui::Ui),
) -> bool {
    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of(id),
        egui::ViewportBuilder::default()
            .with_title(title)
            .with_icon(app_icon())
            .with_inner_size(size)
            .with_min_inner_size(min_size),
        |ui, _class| {
            add(ui);
            !ui.ctx().input(|i| i.viewport().close_requested())
        },
    )
}

/// Small colored dot used in lists.
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
}
