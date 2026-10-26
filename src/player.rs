//! libmpv playback, rendered into the egui window through mpv's OpenGL render API.

use std::ffi::{CStr, CString, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use eframe::egui;
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::{Format, Mpv};

/// Value of an observed property change.
enum PropValue {
    None,
    Str(String),
    Flag(bool),
    Int64(i64),
    Double(f64),
}

impl PropValue {
    /// # Safety
    /// `prop` must come from an `MPV_EVENT_PROPERTY_CHANGE` event that is still valid.
    unsafe fn read(prop: &libmpv2_sys::mpv_event_property) -> Self {
        use libmpv2_sys as sys;
        unsafe {
            match prop.format {
                sys::mpv_format_MPV_FORMAT_STRING => {
                    let p = *(prop.data as *const *const std::ffi::c_char);
                    Self::Str(CStr::from_ptr(p).to_string_lossy().into_owned())
                }
                sys::mpv_format_MPV_FORMAT_FLAG => Self::Flag(*(prop.data as *const i32) != 0),
                sys::mpv_format_MPV_FORMAT_INT64 => Self::Int64(*(prop.data as *const i64)),
                sys::mpv_format_MPV_FORMAT_DOUBLE => Self::Double(*(prop.data as *const f64)),
                _ => Self::None,
            }
        }
    }
}

use crate::log;
use crate::settings::ShotFormat;
use crate::tracks::{Track, TrackKind};

type GetProcAddress = Arc<dyn Fn(&std::ffi::CStr) -> *const c_void + Send + Sync>;

fn get_proc_address(gpa: &GetProcAddress, name: &str) -> *mut c_void {
    let name = CString::new(name).unwrap();
    gpa(&name) as *mut c_void
}

struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    width: i32,
    height: i32,
}

struct GlState {
    render: Option<RenderContext<'static>>,
    target: Option<Target>,
}

/// The render context is only ever touched from eframe's paint callbacks and
/// `on_exit`, which all run on the GL (main) thread.
struct GlCell(Mutex<GlState>);
unsafe impl Send for GlCell {}
unsafe impl Sync for GlCell {}

pub struct Player {
    mpv: &'static Mpv,
    gl: Arc<GlCell>,
    pub file: Option<PathBuf>,
    pub time: f64,
    pub duration: f64,
    pub paused: bool,
    pub tracks: Vec<Track>,
    pub aid: Option<i64>,
    pub sid: Option<i64>,
    pub video_height: Option<u32>,
    pub video_width: Option<u32>,
    pub video_codec: Option<String>,
    /// Frames per second of the video (container value, falling back to 23.976).
    pub fps: f64,
    /// (start time, title)
    pub chapters: Vec<(f64, String)>,
    /// A seek has been sent and mpv hasn't shown the resulting frame yet.
    seek_in_flight: Option<Instant>,
    /// Newest seek requested while one was in flight; older ones are dropped.
    pending_seek: Option<(f64, bool)>,
}

/// Property observation ids.
const OBSERVED: &[(&str, Format)] = &[
    ("time-pos", Format::Double),
    ("duration", Format::Double),
    ("pause", Format::Flag),
    ("aid", Format::String),
    ("sid", Format::String),
    ("width", Format::Int64),
    ("height", Format::Int64),
    ("track-list/count", Format::Int64),
];

/// Give up waiting for a seek to finish after this long (e.g. seek past the end).
const SEEK_TIMEOUT: Duration = Duration::from_millis(400);

impl Player {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: &crate::settings::Settings,
    ) -> Result<Self> {
        let gpa = cc
            .get_proc_address
            .clone()
            .context("eframe did not provide an OpenGL context")?;

        let mut mpv = Mpv::with_initializer(|init| {
            init.set_option("vo", "libmpv")?;
            init.set_option("config", "no")?;
            init.set_option("terminal", "no")?;
            init.set_option("input-default-bindings", "no")?;
            init.set_option("hwdec", if settings.hwdec { "auto-safe" } else { "no" })?;
            init.set_option("keep-open", "yes")?;
            init.set_option("idle", "yes")?;
            init.set_option("hr-seek", "yes")?;
            init.set_option("audio-display", "no")?;
            init.set_option("osd-level", "0")?;
            init.set_option("alang", settings.audio_langs.as_str())?;
            init.set_option("slang", settings.sub_langs.as_str())?;
            init.set_option("sub-auto", "fuzzy")?;
            // Keep recently decoded packets so scrubbing backwards doesn't re-read the file.
            init.set_option("demuxer-max-back-bytes", "150MiB")?;
            // Don't let mpv_render_context_render block the UI thread waiting for vsync timing.
            init.set_option("video-timing-offset", "0")?;
            init.set_option("screenshot-high-bit-depth", "no")?;
            init.set_option("screenshot-jpeg-quality", "95")?;
            init.set_option("screenshot-webp-quality", "92")?;
            Ok(())
        })
        .map_err(|e| anyhow!("failed to start libmpv: {e}"))?;

        let ctx = cc.egui_ctx.clone();
        mpv.set_wakeup_callback(move || ctx.request_repaint());

        // The player lives for the whole process; leaking gives the render context its 'static borrow.
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        // libmpv2 doesn't wrap this call; route mpv's warnings into our log.
        unsafe {
            libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), c"warn".as_ptr());
        }
        // State arrives as events instead of being polled every frame: a synchronous
        // get_property has to wait for mpv's core, which stalls the UI while it decodes.
        for (i, (name, format)) in OBSERVED.iter().enumerate() {
            mpv.observe_property(name, *format, i as u64)
                .map_err(|e| anyhow!("observe {name}: {e}"))?;
        }

        let mut render = mpv
            .create_render_context([
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address,
                    ctx: gpa,
                }),
            ])
            .map_err(|e| anyhow!("failed to create mpv render context: {e}"))?;
        let ctx = cc.egui_ctx.clone();
        render.set_update_callback(move || ctx.request_repaint());

        Ok(Self {
            mpv,
            gl: Arc::new(GlCell(Mutex::new(GlState {
                render: Some(render),
                target: None,
            }))),
            file: None,
            time: 0.0,
            duration: 0.0,
            paused: true,
            tracks: Vec::new(),
            aid: None,
            sid: None,
            video_height: None,
            video_width: None,
            video_codec: None,
            fps: 24000.0 / 1001.0,
            chapters: Vec::new(),
            seek_in_flight: None,
            pending_seek: None,
        })
    }

    /// Run an mpv command without waiting for mpv's core.
    fn command_async(&self, args: &[&str]) {
        let cstrs: Vec<CString> = match args.iter().map(|a| CString::new(*a)).collect() {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut ptrs: Vec<*const std::ffi::c_char> = cstrs.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let err =
            unsafe { libmpv2_sys::mpv_command_async(self.mpv.ctx.as_ptr(), 0, ptrs.as_mut_ptr()) };
        if err < 0 {
            log::warn("mpv", format!("command {args:?} failed: {err}"));
        }
    }

    pub fn load(&mut self, path: &Path) -> Result<()> {
        let s = path.to_str().context("path is not valid UTF-8")?;
        self.mpv
            .command("loadfile", &[s])
            .map_err(|e| anyhow!("mpv could not open file: {e}"))?;
        log::info("player", format!("opening {}", path.display()));
        self.file = Some(path.to_path_buf());
        self.tracks.clear();
        self.chapters.clear();
        self.seek_in_flight = None;
        self.pending_seek = None;
        self.set_pause(true);
        Ok(())
    }

    pub fn close(&mut self) {
        self.command_async(&["stop"]);
        self.file = None;
        self.tracks.clear();
        self.chapters.clear();
        self.time = 0.0;
        self.duration = 0.0;
        self.aid = None;
        self.sid = None;
        self.video_height = None;
        self.video_width = None;
        self.video_codec = None;
    }

    /// Apply mpv's queued events. Call once per UI frame.
    pub fn poll(&mut self) {
        use libmpv2_sys as sys;
        let mut file_loaded = false;
        let mut reload_tracks = false;
        loop {
            // Read events through the C API: libmpv2's wrapper reports "property became
            // unavailable" the same way as "queue empty".
            let ev = unsafe { &*sys::mpv_wait_event(self.mpv.ctx.as_ptr(), 0.0) };
            match ev.event_id {
                sys::mpv_event_id_MPV_EVENT_NONE => break,
                sys::mpv_event_id_MPV_EVENT_PROPERTY_CHANGE => {
                    let prop = unsafe { &*(ev.data as *const sys::mpv_event_property) };
                    let name = unsafe { CStr::from_ptr(prop.name) }.to_str().unwrap_or("");
                    let value = unsafe { PropValue::read(prop) };
                    match (name, value) {
                        ("time-pos", PropValue::Double(v)) if self.seek_in_flight.is_none() => {
                            self.time = v
                        }
                        ("duration", PropValue::Double(v)) => self.duration = v,
                        ("pause", PropValue::Flag(b)) => self.paused = b,
                        ("aid", PropValue::Str(s)) => self.aid = s.parse().ok(),
                        ("sid", PropValue::Str(s)) => self.sid = s.parse().ok(),
                        ("width", PropValue::Int64(v)) => self.video_width = Some(v as u32),
                        ("height", PropValue::Int64(v)) => self.video_height = Some(v as u32),
                        ("track-list/count", _) => reload_tracks = true,
                        _ => {}
                    }
                }
                sys::mpv_event_id_MPV_EVENT_PLAYBACK_RESTART => {
                    self.seek_finished();
                    // time-pos is reported before the restart; pick up the final position.
                    if let Ok(t) = self.mpv.get_property::<f64>("time-pos") {
                        self.time = t;
                    }
                }
                sys::mpv_event_id_MPV_EVENT_FILE_LOADED => file_loaded = true,
                sys::mpv_event_id_MPV_EVENT_LOG_MESSAGE => {
                    let m = unsafe { &*(ev.data as *const sys::mpv_event_log_message) };
                    let text = |p| unsafe { CStr::from_ptr(p) }.to_string_lossy();
                    let lvl = match text(m.level).as_ref() {
                        "fatal" | "error" => log::Level::Error,
                        "warn" => log::Level::Warn,
                        _ => log::Level::Debug,
                    };
                    log::log(
                        lvl,
                        "mpv",
                        format!("[{}] {}", text(m.prefix), text(m.text).trim_end()),
                    );
                }
                _ => {}
            }
        }

        if self
            .seek_in_flight
            .is_some_and(|t| t.elapsed() > SEEK_TIMEOUT)
        {
            self.seek_finished();
        }
        if reload_tracks || file_loaded {
            self.tracks = self.read_tracks();
        }
        if file_loaded {
            self.on_file_loaded();
        }
    }

    fn on_file_loaded(&mut self) {
        self.fps = self
            .mpv
            .get_property::<f64>("container-fps")
            .ok()
            .filter(|f| *f > 1.0 && *f < 500.0)
            .unwrap_or(24000.0 / 1001.0);
        self.video_codec = self.mpv.get_property::<String>("video-codec").ok();
        let n = self
            .mpv
            .get_property::<i64>("chapter-list/count")
            .unwrap_or(0);
        self.chapters = (0..n)
            .filter_map(|i| {
                let t = self
                    .mpv
                    .get_property::<f64>(&format!("chapter-list/{i}/time"))
                    .ok()?;
                let title = self
                    .mpv
                    .get_property::<String>(&format!("chapter-list/{i}/title"))
                    .unwrap_or_else(|_| format!("Chapter {}", i + 1));
                Some((t, title))
            })
            .collect();
        log::info(
            "player",
            format!(
                "loaded: {} @ {:.3} fps, {} tracks, {} chapters",
                self.video_codec.as_deref().unwrap_or("?"),
                self.fps,
                self.tracks.len(),
                self.chapters.len()
            ),
        );
    }

    /// Start/end of the subtitle line currently on screen.
    pub fn current_sub_line(&self) -> Option<(f64, f64)> {
        let start = self.mpv.get_property::<f64>("sub-start").ok()?;
        let end = self.mpv.get_property::<f64>("sub-end").ok()?;
        (end > start).then_some((start, end))
    }

    /// Jump to the previous/next subtitle line.
    pub fn sub_seek(&mut self, forward: bool) {
        self.command_async(&["sub-seek", if forward { "1" } else { "-1" }]);
    }

    pub fn set_hwdec(&self, on: bool) {
        let _ = self
            .mpv
            .set_property("hwdec", if on { "auto-safe" } else { "no" });
    }

    pub fn set_langs(&self, audio: &str, sub: &str) {
        let _ = self.mpv.set_property("alang", audio);
        let _ = self.mpv.set_property("slang", sub);
    }

    fn read_tracks(&self) -> Vec<Track> {
        let count = self
            .mpv
            .get_property::<i64>("track-list/count")
            .unwrap_or(0);
        let s = |i: i64, k: &str| {
            self.mpv
                .get_property::<String>(&format!("track-list/{i}/{k}"))
                .ok()
                .filter(|v| !v.is_empty())
        };
        let n = |i: i64, k: &str| {
            self.mpv
                .get_property::<i64>(&format!("track-list/{i}/{k}"))
                .ok()
        };

        (0..count)
            .filter_map(|i| {
                let kind = match s(i, "type")?.as_str() {
                    "video" => TrackKind::Video,
                    "audio" => TrackKind::Audio,
                    "sub" => TrackKind::Sub,
                    _ => return None,
                };
                let external = self
                    .mpv
                    .get_property::<bool>(&format!("track-list/{i}/external"))
                    .unwrap_or(false);
                Some(Track {
                    kind,
                    id: n(i, "id")?,
                    ff_index: n(i, "ff-index"),
                    lang: s(i, "lang"),
                    title: s(i, "title"),
                    codec: s(i, "codec").unwrap_or_else(|| "?".into()),
                    external: external
                        .then(|| s(i, "external-filename").map(PathBuf::from))
                        .flatten(),
                    channels: n(i, "demux-channel-count"),
                })
            })
            .collect()
    }

    pub fn current(&self, kind: TrackKind) -> Option<&Track> {
        let id = match kind {
            TrackKind::Audio => self.aid?,
            TrackKind::Sub => self.sid?,
            TrackKind::Video => return self.tracks.iter().find(|t| t.kind == kind),
        };
        self.tracks.iter().find(|t| t.kind == kind && t.id == id)
    }

    pub fn select_track(&self, kind: TrackKind, id: Option<i64>) {
        let prop = match kind {
            TrackKind::Audio => "aid",
            TrackKind::Sub => "sid",
            TrackKind::Video => "vid",
        };
        let value = id.map_or_else(|| "no".to_string(), |id| id.to_string());
        let _ = self.mpv.set_property(prop, value.as_str());
    }

    pub fn set_pause(&self, pause: bool) {
        let _ = self.mpv.set_property("pause", pause);
    }

    pub fn toggle_pause(&self) {
        self.command_async(&["cycle", "pause"]);
    }

    /// Seek to `t`. While scrubbing, only one seek is outstanding at a time and only
    /// the newest target is kept, so mpv never builds up a backlog.
    /// `exact = false` lands on the nearest keyframe, which is much faster.
    pub fn seek(&mut self, t: f64, exact: bool) {
        let t = t.clamp(0.0, self.duration.max(0.0));
        if self.seek_in_flight.is_some() {
            // A precise request must not be downgraded by a later fast one for the same spot.
            self.pending_seek = Some((t, exact));
            return;
        }
        self.send_seek(t, exact);
    }

    fn send_seek(&mut self, t: f64, exact: bool) {
        let mode = if exact {
            "absolute+exact"
        } else {
            "absolute+keyframes"
        };
        self.command_async(&["seek", &format!("{t:.4}"), mode]);
        // Show the target immediately; mpv's time-pos follows when the frame is ready.
        self.time = t;
        self.seek_in_flight = Some(Instant::now());
    }

    fn seek_finished(&mut self) {
        self.seek_in_flight = None;
        if let Some((t, exact)) = self.pending_seek.take() {
            self.send_seek(t, exact);
        }
    }

    pub fn seek_relative(&mut self, delta: f64) {
        self.command_async(&["seek", &format!("{delta:.4}"), "relative+exact"]);
    }

    /// Step `n` frames (negative = backwards). One frame uses mpv's frame stepping;
    /// more is an exact seek to the target frame.
    pub fn step_frames(&mut self, n: i64) {
        match n {
            1 => self.command_async(&["frame-step"]),
            -1 => self.command_async(&["frame-back-step"]),
            0 => {}
            _ => {
                let frame = (self.time * self.fps).round() + n as f64;
                self.seek(frame.max(0.0) / self.fps, true);
            }
        }
    }

    pub fn set_volume(&self, volume: f64) {
        let _ = self.mpv.set_property("volume", volume);
    }

    /// Playback speed multiplier (1.0 = normal).
    pub fn set_speed(&self, speed: f64) {
        let _ = self.mpv.set_property("speed", speed);
    }

    pub fn set_mute(&self, mute: bool) {
        let _ = self.mpv.set_property("mute", mute);
    }

    /// Show or hide subtitles in the preview (doesn't change the selected track).
    pub fn set_sub_visibility(&self, visible: bool) {
        let _ = self.mpv.set_property("sub-visibility", visible);
    }

    /// Preview framing: `zoom` is log2 relative to fit-to-window, `panscan` 1.0 crops to
    /// fill, `pan` is in fractions of the displayed video, `aspect` e.g. "16:9" or "no" (from the file).
    pub fn set_view(&self, zoom: f64, panscan: f64, pan: (f64, f64), aspect: &str) {
        let _ = self.mpv.set_property("video-zoom", zoom);
        let _ = self.mpv.set_property("panscan", panscan);
        let _ = self.mpv.set_property("video-pan-x", pan.0);
        let _ = self.mpv.set_property("video-pan-y", pan.1);
        let _ = self.mpv.set_property("video-aspect-override", aspect);
    }

    /// Loop playback between a and b (both `None` disables the loop).
    pub fn set_ab_loop(&self, ab: Option<(f64, f64)>) {
        match ab {
            Some((a, b)) => {
                let _ = self.mpv.set_property("ab-loop-a", a);
                let _ = self.mpv.set_property("ab-loop-b", b);
            }
            None => {
                let _ = self.mpv.set_property("ab-loop-a", "no");
                let _ = self.mpv.set_property("ab-loop-b", "no");
            }
        }
    }

    /// Save the current frame at source resolution, exactly as mpv renders it.
    pub fn screenshot(&self, path: &Path, format: ShotFormat, with_subs: bool) -> Result<()> {
        let s = path.to_str().context("path is not valid UTF-8")?;
        self.mpv
            .set_property("screenshot-format", format.ext())
            .map_err(|e| anyhow!("{e}"))?;
        // "Burn in" must include subtitles even when they're hidden in the preview.
        let hidden = with_subs
            && !self
                .mpv
                .get_property::<bool>("sub-visibility")
                .unwrap_or(true);
        if hidden {
            self.set_sub_visibility(true);
        }
        let result = self
            .mpv
            .command(
                "screenshot-to-file",
                &[s, if with_subs { "subtitles" } else { "video" }],
            )
            .map_err(|e| anyhow!("screenshot failed: {e}"));
        if hidden {
            self.set_sub_visibility(false);
        }
        result
    }

    /// Draw the video into `rect`.
    pub fn paint(&self, ui: &egui::Ui, rect: egui::Rect) {
        let gl = self.gl.clone();
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            gl.paint(&info, painter);
        });
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });
    }

    /// Free GL resources while the context is still alive.
    pub fn destroy_gl(&self, gl: Option<&glow::Context>) {
        let mut state = self.gl.0.lock().unwrap();
        state.render = None;
        if let (Some(gl), Some(t)) = (gl, state.target.take()) {
            unsafe {
                gl.delete_framebuffer(t.fbo);
                gl.delete_texture(t.tex);
            }
        }
    }
}

impl GlCell {
    fn paint(&self, info: &egui::PaintCallbackInfo, painter: &egui_glow::Painter) {
        let gl = painter.gl();
        let vp = info.viewport_in_pixels();
        if vp.width_px <= 0 || vp.height_px <= 0 {
            return;
        }
        let mut state = self.0.lock().unwrap();
        let GlState { render, target } = &mut *state;
        let Some(render) = render.as_ref() else {
            return;
        };

        unsafe {
            if target
                .as_ref()
                .is_none_or(|t| t.width != vp.width_px || t.height != vp.height_px)
            {
                if let Some(old) = target.take() {
                    gl.delete_framebuffer(old.fbo);
                    gl.delete_texture(old.tex);
                }
                match create_target(gl, vp.width_px, vp.height_px) {
                    Ok(t) => *target = Some(t),
                    Err(e) => {
                        eprintln!("video target: {e}");
                        return;
                    }
                }
            }
            let t = target.as_ref().unwrap();

            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            let fbo_id = t.fbo.0.get() as i32;
            if let Err(e) = render.render::<GetProcAddress>(fbo_id, t.width, t.height, true) {
                eprintln!("mpv render: {e}");
            }

            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(t.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, painter.intermediate_fbo());
            gl.blit_framebuffer(
                0,
                0,
                t.width,
                t.height,
                vp.left_px,
                vp.from_bottom_px,
                vp.left_px + t.width,
                vp.from_bottom_px + t.height,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
        }
    }
}

unsafe fn create_target(gl: &glow::Context, width: i32, height: i32) -> Result<Target, String> {
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width,
            height,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);

        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(tex),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(tex);
            return Err(format!("framebuffer incomplete: {status:#x}"));
        }
        Ok(Target {
            fbo,
            tex,
            width,
            height,
        })
    }
}
