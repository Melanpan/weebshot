//! MCP commands are applied on the GUI thread using the same actions as the interface.
use super::*;
use crate::mcp::{
    self, Request,
    ipc::Bridge,
    service::{ContextSnapshot, Service},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct State {
    pub(super) media_revision: u64,
    bridge: Option<Bridge>,
    service: Option<Arc<Service>>,
    error: Option<String>,
    attempted: bool,
    mirrored: BTreeMap<u64, u64>,
    prepare: Option<(PathBuf, f64, f64, bool, Settings, Instant)>,
}
impl App {
    pub(super) fn mcp_page(&mut self, ui: &mut egui::Ui) {
        use super::settings_ui::{Setting, code, group, row};
        group(ui, "Automation / MCP");
        row(ui, Setting::Mcp, |ui| {
            ui.label("Optional local automation for media processing and this window.");
            if ui
                .checkbox(&mut self.settings.mcp_enabled, "Enable MCP integration")
                .changed()
            {
                self.automation.attempted = false;
            }
            ui.label("Applies immediately. Disabling stops accepting requests and cancels automation jobs.");
            if let Some(error) = &self.automation.error {
                ui.colored_label(self.palette.error, error);
            }
        });
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("weebshot"));
        // JSON quoting works for client configuration; shell commands need platform quoting.
        let command = super::options::shell_executable(&exe);
        row(ui, Setting::GuiCommand, |ui| {
            code(ui, "GUI attachment command", &format!("{command} --mcp"));
            ui.label("Attaches clients to the running GUI through the local socket.");
        });
        row(ui, Setting::HeadlessCommand, |ui| {
            code(
                ui,
                "Headless launch command",
                &format!("{command} --headless --mcp"),
            );
            ui.label("Runs media processing without opening a window.");
        });
        row(ui, Setting::Socket, |ui| match mcp::ipc::default_path() {
            Ok(socket) => code(ui, "Local socket path", &socket.display().to_string()),
            Err(error) => {
                ui.colored_label(self.palette.error, error.to_string());
            }
        });
        row(ui, Setting::McpConfig, |ui| {
            if ui.button("Copy MCP client configuration").clicked() {
                ui.ctx().copy_text(
                    serde_json::to_string_pretty(
                        &json!({"mcpServers":{"weebshot":{"command":exe,"args":["--mcp"]}}}),
                    )
                    .unwrap(),
                );
            }
            ui.label("Only connect trusted local clients. They can read local media and create output files with your permissions.");
            ui.hyperlink_to(
                "MCP documentation",
                "https://github.com/Melanpan/weebshot/blob/main/docs/mcp.md",
            );
        });
    }
    pub(super) fn poll_automation(&mut self, ctx: &egui::Context) {
        if !self.settings.mcp_enabled {
            if self.automation.bridge.is_some() || self.automation.service.is_some() {
                self.automation.bridge = None;
                self.automation.service = None;
                self.automation.prepare = None;
                for view_id in self.automation.mirrored.values() {
                    if let Some(view) = self.jobs.iter_mut().find(|j| j.id == *view_id)
                        && matches!(view.state, JobState::Queued | JobState::Running { .. })
                    {
                        view.state = JobState::Failed("MCP disabled; job cancelled".into());
                    }
                }
                self.automation.mirrored.clear();
            }
            self.automation.attempted = false;
            self.automation.error = None;
            return;
        }
        if !self.automation.attempted {
            self.automation.attempted = true;
            let service = Arc::new(Service::new(self.tools.clone()));
            match mcp::ipc::default_path()
                .and_then(|p| Bridge::with_service(p, ctx.clone(), Some(service.clone())))
            {
                Ok(bridge) => {
                    self.automation.bridge = Some(bridge);
                    self.automation.service = Some(service);
                    self.automation.error = None;
                }
                Err(e) => self.automation.error = Some(format!("{e:#}")),
            }
        }
        if let Some((path, start, end, seek, settings, deadline)) = self.automation.prepare.take() {
            if self.player.file.as_ref() != Some(&path) {
                self.automation.error = Some("Prepared export cancelled: media changed".into());
            } else if self.in_point.is_some() || self.out_point.is_some() {
                self.automation.error = Some(
                    "Prepared export cancelled: selection changed while media was loading".into(),
                );
            } else if self.player.duration > 0.0 {
                match mcp::range(start, end, self.player.duration) {
                    Ok(_) => {
                        self.set_range(start, end);
                        self.settings = settings;
                        if seek {
                            self.player.seek(start, true);
                        }
                        self.automation.error = None;
                    }
                    Err(e) => self.automation.error = Some(e.to_string()),
                }
            } else if Instant::now() < deadline {
                self.automation.prepare = Some((path, start, end, seek, settings, deadline));
            } else {
                self.automation.error =
                    Some("Media did not finish loading; preparation cancelled".into());
            }
        }
        for _ in 0..16 {
            let pending = self
                .automation
                .bridge
                .as_ref()
                .and_then(|b| b.requests.try_recv().ok());
            let Some(pending) = pending else { break };
            if !pending.valid() {
                continue;
            }
            if mcp::service::is_submission(&pending.request.name) {
                match self.automation_context(&pending.request) {
                    Ok(snapshot) => self
                        .automation
                        .service
                        .as_ref()
                        .unwrap()
                        .submit(snapshot, pending),
                    Err(error) => {
                        let _ = pending.reply.send(Err(format!("{error:#}")));
                    }
                }
                continue;
            }
            let result = self
                .automation_call(&pending.request)
                .map_err(|e| format!("{e:#}"));
            let _ = pending.reply.send(result);
        }
        if let Some(service) = &self.automation.service {
            for (id, state, cancel) in service.snapshots() {
                let view_id = *self.automation.mirrored.entry(id).or_insert_with(|| {
                    let view_id = self.next_job_id;
                    self.next_job_id += 1;
                    let output = state["destination"].as_str().map(PathBuf::from);
                    self.jobs.push(JobView {
                        id: view_id,
                        name: output
                            .as_deref()
                            .and_then(Path::file_name)
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "MCP export".into()),
                        output,
                        details: Some("MCP".into()),
                        retry: None,
                        progress: 0.0,
                        state: JobState::Queued,
                        cancel,
                        started: None,
                        elapsed: None,
                        size: None,
                    });
                    view_id
                });
                if let Some(view) = self.jobs.iter_mut().find(|j| j.id == view_id) {
                    view.progress = state["progress"].as_f64().unwrap_or(0.0) as f32;
                    view.state = match state["status"].as_str().unwrap_or("queued") {
                        "done" => JobState::Done,
                        "failed" | "cancelled" => {
                            JobState::Failed(state["error"].as_str().unwrap_or("cancelled").into())
                        }
                        "running" => {
                            view.started.get_or_insert_with(Instant::now);
                            JobState::Running {
                                stage: "MCP media processing",
                                step: Some(view.progress),
                            }
                        }
                        _ => JobState::Queued,
                    };
                    if matches!(view.state, JobState::Done | JobState::Failed(_)) {
                        view.elapsed = view.started.map(|s| s.elapsed());
                    }
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    fn automation_state(&self) -> Value {
        json!({"output_base_directory":std::env::current_dir().ok(),"media_revision":self.automation.media_revision,"ffmpeg_path":self.tools.ffmpeg,"gui":true,"media":self.player.file,"position_seconds":self.player.time,"duration_seconds":self.player.duration,"fps":self.player.fps,"paused":self.player.paused,"in_seconds":self.in_point,"out_seconds":self.out_point,"tracks":self.automation_tracks(),"settings":mcp::export_settings(&self.settings),"preparing":self.automation.prepare.is_some(),"automation_error":self.automation.error,"jobs":self.jobs.iter().map(|j|json!({"job_id":j.id,"output":j.output,"progress":j.progress,"status":match j.state{JobState::Queued=>"queued",JobState::Running{..}=>"running",JobState::Done=>"done",JobState::Failed(_)=>"failed"}})).collect::<Vec<_>>()})
    }
    fn automation_tracks(&self) -> Value {
        mcp::identifiers::gui_tracks(&self.player.tracks, self.player.aid, self.player.sid)
    }
    fn automation_protect(&self, a: &Value) -> Result<()> {
        ensure!(
            a["replace_selection"] == true
                || (self.in_point.is_none()
                    && self.out_point.is_none()
                    && self.automation.prepare.is_none()),
            "Existing selection would be replaced; pass replace_selection=true"
        );
        ensure!(
            a["replace_selection"] == true || !self.cues.as_ref().is_some_and(|c| c.dirty),
            "Edited subtitles are unsaved; pass replace_selection=true to replace media"
        );
        Ok(())
    }
    fn automation_time(&self, a: &Value) -> Result<f64> {
        ensure!(
            a.get("seconds").is_none() || a.get("frame").is_none(),
            "Specify seconds or frame, not both"
        );
        let time = a["seconds"]
            .as_f64()
            .or_else(|| a["frame"].as_u64().map(|f| f as f64 / self.player.fps))
            .context("Specify seconds or frame")?;
        mcp::position(time, self.player.duration)
    }
    fn automation_context(&self, request: &Request) -> Result<ContextSnapshot> {
        let tracks = self.automation_tracks();
        let needs_subs = match request.name.as_str() {
            "screenshot_extract" => request.arguments["subtitles"]
                .as_bool()
                .unwrap_or(self.settings.shot_subs != SubMode::Off),
            "export_add" => request.arguments["settings"]["clip_subs"]
                .as_str()
                .map_or(self.settings.clip_subs != SubMode::Off, |s| s != "Off"),
            _ => false,
        };
        ensure!(
            !needs_subs
                || self.player.sid.is_none()
                || self.player.current(TrackKind::Sub).is_some(),
            "Selected subtitle track is not ready; query media_tracks and retry"
        );
        let subtitles = if needs_subs && self.player.file.is_some() {
            Some(self.sub_source().map_err(anyhow::Error::msg)?)
        } else {
            Some(None)
        };
        Ok(ContextSnapshot {
            tools: self.tools.clone(),
            media: self.player.file.clone().map(|path| mcp::Media {
                path,
                info: json!({"streams":tracks}),
                duration: self.player.duration,
                fps: self.player.fps,
                width: self.player.video_width.unwrap_or(0),
                height: self.player.video_height.unwrap_or(0),
                audio: self
                    .player
                    .current(TrackKind::Audio)
                    .and_then(|t| t.ff_index),
                subtitle: None,
                external_subtitle: None,
            }),
            settings: self.settings.clone(),
            time: self.player.time,
            in_point: self.in_point,
            out_point: self.out_point,
            subtitles,
        })
    }
    fn automation_call(&mut self, request: &Request) -> Result<Value> {
        let a = &request.arguments;
        let name = match request.name.as_str() {
            "gui_open_media" => "media_open",
            "gui_seek" => "timeline_seek",
            "gui_set_range" => "timeline_set_range",
            "gui_set_export_settings" => "export_set_settings",
            n => n,
        };
        match name {
            "gui_get_state" | "timeline_get_state" => Ok(self.automation_state()),
            "media_open" => {
                self.automation_protect(a)?;
                let path = mcp::input_path(a["path"].as_str().unwrap())?;
                self.automation.prepare = None;
                self.open(&path);
                Ok(self.automation_state())
            }
            "media_close" => {
                self.automation_protect(a)?;
                self.automation.prepare = None;
                self.close();
                Ok(self.automation_state())
            }
            "media_info" => {
                ensure!(self.player.file.is_some(), "Open media first");
                Ok(
                    json!({"path":self.player.file,"duration_seconds":self.player.duration,"fps":self.player.fps,"width":self.player.video_width,"height":self.player.video_height,"codec":self.player.video_codec,"track_id_namespace":"mpv","tracks":self.automation_tracks()}),
                )
            }
            "media_tracks" => Ok(
                json!({"tracks":self.automation_tracks(),"audio":self.player.aid,"subtitle":self.player.sid}),
            ),
            "media_select_track" => {
                ensure!(self.player.file.is_some(), "Open media first");
                let kind = if a["kind"] == "audio" {
                    TrackKind::Audio
                } else {
                    TrackKind::Sub
                };
                ensure!(
                    a.get("track_id").is_some() != a.get("external_path").is_some(),
                    "Specify exactly one of track_id (null disables) or external_path"
                );
                if let Some(path) = a["external_path"].as_str() {
                    ensure!(
                        kind == TrackKind::Sub,
                        "external_path is only supported for subtitles"
                    );
                    let path = mcp::input_path(path)?;
                    self.player.add_sub_checked(&path)?;
                    return Ok(
                        json!({"kind":"subtitle","track_id":self.player.sid,"mpv_id":self.player.sid,"id_namespace":"mpv","external":path,"tracks":self.automation_tracks()}),
                    );
                }
                let id = a["track_id"].as_i64();
                ensure!(
                    a["track_id"].is_null() || id.is_some(),
                    "track_id must fit in a signed 64-bit integer"
                );
                let namespace = a["id_namespace"].as_str().unwrap_or("native");
                let mpv_id = mcp::identifiers::gui_id(&self.player.tracks, kind, id, namespace)?;
                self.player.select_track_checked(kind, mpv_id)?;
                Ok(
                    json!({"kind":a["kind"],"track_id":mpv_id,"mpv_id":mpv_id,"id_namespace":"mpv","tracks":self.automation_tracks()}),
                )
            }
            "timeline_seek" => {
                let t = self.automation_time(a)?;
                self.stop_shuttle();
                self.play_until = None;
                self.player.seek(t, true);
                Ok(self.automation_state())
            }
            "timeline_set_range" | "timeline_set_in" | "timeline_set_out" => {
                self.automation_protect(a)?;
                let start = if name == "timeline_set_in" {
                    Some(a["seconds"].as_f64().unwrap_or(self.player.time))
                } else {
                    a["in_seconds"].as_f64().or(self.in_point)
                };
                let end = if name == "timeline_set_out" {
                    Some(a["seconds"].as_f64().unwrap_or(self.player.time))
                } else {
                    a["out_seconds"].as_f64().or(self.out_point)
                };
                if let Some(t) = start {
                    mcp::position(t, self.player.duration)?;
                }
                if let Some(t) = end {
                    mcp::position(t, self.player.duration)?;
                }
                if let (Some(s), Some(e)) = (start, end) {
                    mcp::range(s, e, self.player.duration)?;
                }
                self.automation.prepare = None;
                self.in_point = start;
                self.out_point = end;
                self.set_looping(self.looping);
                if a["seek_to_in"] == true {
                    self.player.seek(start.unwrap_or(self.player.time), true);
                }
                Ok(self.automation_state())
            }
            "playback_control" => {
                self.stop_shuttle();
                self.play_until = None;
                ensure!(self.player.file.is_some(), "Open media first");
                match a["action"].as_str().unwrap() {
                    "play" => self.player.set_pause(false),
                    "pause" => self.player.set_pause(true),
                    "step" => {
                        let n = a["frames"].as_i64().unwrap_or(1);
                        mcp::position(
                            self.player.time + n as f64 / self.player.fps,
                            self.player.duration,
                        )?;
                        self.player.step_frames(n);
                    }
                    _ => unreachable!(),
                };
                Ok(self.automation_state())
            }
            "export_get_settings" => Ok(mcp::export_settings(&self.settings)),
            "export_set_settings" => {
                self.settings = mcp::update_settings(&self.settings, &a["settings"])?;
                Ok(mcp::export_settings(&self.settings))
            }
            "gui_prepare_export" => {
                self.automation_protect(a)?;
                let settings = if let Some(patch) = a.get("settings") {
                    mcp::update_settings(&self.settings, patch)?
                } else {
                    self.settings.clone()
                };
                let start = a["in_seconds"].as_f64().unwrap();
                let end = a["out_seconds"].as_f64().unwrap();
                let seek = a["seek_to_in"].as_bool().unwrap_or(false);
                let path = if let Some(path) = a["media"].as_str() {
                    mcp::input_path(path)?
                } else {
                    self.player
                        .file
                        .clone()
                        .context("Open media or supply media")?
                };
                if self.player.file.as_ref() != Some(&path) {
                    self.open(&path);
                    self.automation.prepare = Some((
                        path,
                        start,
                        end,
                        seek,
                        settings,
                        Instant::now() + Duration::from_secs(10),
                    ));
                } else {
                    mcp::range(start, end, self.player.duration)?;
                    self.set_range(start, end);
                    self.settings = settings;
                    if seek {
                        self.player.seek(start, true);
                    }
                }
                Ok(self.automation_state())
            }
            _ => anyhow::bail!("Unknown GUI command"),
        }
    }
}
