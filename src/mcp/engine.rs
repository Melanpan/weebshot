use super::Request;
use crate::{
    export::{
        self, ClipJob, Crop, ExportJob, JobEvent, Runner, Scratch, SubSource, Tools, anime4k,
        ffmpeg::StillJob,
    },
    settings::Settings,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub fn input_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path)
        .canonicalize()
        .context("Media must be an existing local file")?;
    ensure!(path.is_file(), "Media must be a regular file");
    Ok(path)
}
fn output_path(path: &str, base: Option<&str>) -> Result<(PathBuf, PathBuf)> {
    let base = if let Some(base) = base {
        let base = Path::new(base);
        ensure!(
            base.is_absolute(),
            "base_directory must be an absolute existing directory"
        );
        base.canonicalize()
            .with_context(|| format!("Base directory is unavailable: {}", base.display()))?
    } else {
        std::env::current_dir().context("Cannot determine the Weebshot working directory")?
    };
    ensure!(
        base.is_dir(),
        "Base directory is not a directory: {}",
        base.display()
    );
    let requested = Path::new(path);
    let path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };
    let name = path.file_name().context("Output must have a filename")?;
    let parent = path.parent().context("Output needs a parent directory")?;
    let parent=parent.canonicalize().with_context(||format!("Output directory does not exist or cannot be accessed: {} (relative output base: {}; supply an absolute output or explicit base_directory)",parent.display(),base.display()))?;
    ensure!(
        parent.is_dir(),
        "Output parent is not a directory: {}",
        parent.display()
    );
    let output = parent.join(name);
    match std::fs::symlink_metadata(&output) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => bail!(
            "Output already exists: {}; choose a new filename",
            output.display()
        ),
        Err(e) => bail!("Cannot inspect output {}: {e}", output.display()),
    }
    Ok((output, base))
}
pub fn position(time: f64, duration: f64) -> Result<f64> {
    ensure!(
        time.is_finite() && time >= 0.0 && time <= duration && duration > 0.0,
        "Timestamp must be between 0 and media duration ({duration})"
    );
    Ok(time)
}
pub fn range(start: f64, end: f64, duration: f64) -> Result<(f64, f64)> {
    position(start, duration)?;
    position(end, duration)?;
    ensure!(end > start, "Out must be after In");
    Ok((start, end))
}
fn merge(target: &mut Value, patch: &Value) {
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            if value.is_object() && target.get(key).is_some_and(Value::is_object) {
                merge(target.get_mut(key).unwrap(), value);
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}
pub fn update_settings(settings: &Settings, patch: &Value) -> Result<Settings> {
    // Validate even when invoked directly by the GUI IPC bridge.
    super::tools::validate(&Request {
        name: "export_set_settings".into(),
        arguments: json!({"settings":patch}),
    })?;
    let mut data = serde_json::to_value(settings)?;
    if patch.get("quality").is_some() && patch.get("anim").is_none() {
        let q = serde_json::from_value(patch["quality"].clone())?;
        data["anim"] = serde_json::to_value(export::AnimOptions::preset(q))?;
    }
    merge(&mut data, patch);
    Ok(serde_json::from_value(data)?)
}
pub fn export_settings(settings: &Settings) -> Value {
    let data = serde_json::to_value(settings).unwrap();
    let props = super::tools::settings_schema()["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    Value::Object(
        props
            .into_iter()
            .map(|k| {
                let v = data[&k].clone();
                (k, v)
            })
            .collect(),
    )
}

pub struct Media {
    pub path: PathBuf,
    pub info: Value,
    pub duration: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub audio: Option<i64>,
    pub subtitle: Option<i64>,
    pub external_subtitle: Option<PathBuf>,
}
impl Media {
    pub(super) fn probe(path: PathBuf) -> Result<Self> {
        Self::probe_with_cancel(path, &AtomicBool::new(false))
    }
    pub(super) fn probe_with_cancel(path: PathBuf, cancel: &AtomicBool) -> Result<Self> {
        let mut cmd = Command::new(export::find_tool("ffprobe"));
        cmd.args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-show_entries",
            "format=duration,format_name,size:stream=index,codec_type,codec_name,width,height,sample_aspect_ratio,avg_frame_rate,duration,channels:stream_tags=language,title,rotate:stream_side_data=rotation",
            "-of",
            "json",
        ])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        export::hide_console(&mut cmd);
        let mut child = cmd
            .spawn()
            .context("ffprobe is required for media metadata")?;
        use std::io::Read;
        let stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None)
                    if std::time::Instant::now() < deadline && !cancel.load(Ordering::Relaxed) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break match result {
                        Err(e) => Err(anyhow::Error::from(e)),
                        _ => Err(anyhow::anyhow!(if cancel.load(Ordering::Relaxed) {
                            "Inspection cancelled"
                        } else {
                            "Media probing timed out"
                        })),
                    };
                }
            }
        };
        let bytes = reader
            .join()
            .map_err(|_| anyhow::anyhow!("Metadata reader stopped"))??;
        ensure!(
            status?.success(),
            "Could not probe media; verify the file is a supported video"
        );
        ensure!(
            bytes.len() <= 4 * 1024 * 1024,
            "Media metadata exceeds the 4 MiB limit"
        );
        let info: Value = serde_json::from_slice(&bytes)?;
        let streams = info["streams"].as_array().context("Media has no streams")?;
        let video = streams
            .iter()
            .find(|s| s["codec_type"] == "video")
            .context("Media contains no video stream")?;
        let duration = info["format"]["duration"]
            .as_str()
            .or_else(|| video["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .context("Media has no finite duration")?;
        ensure!(
            duration.is_finite() && duration > 0.0,
            "Media has invalid duration"
        );
        let fps = video["avg_frame_rate"]
            .as_str()
            .and_then(|s| s.split_once('/'))
            .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
            .filter(|f| f.is_finite() && *f > 0.0)
            .unwrap_or(24.0);
        let width = video["width"].as_u64().context("Missing video width")? as u32;
        let height = video["height"].as_u64().context("Missing video height")? as u32;
        ensure!(
            u64::from(width) * u64::from(height) <= 64_000_000,
            "Video exceeds automation pixel budget"
        );
        let first = |kind| {
            streams
                .iter()
                .find(|s| s["codec_type"] == kind)
                .and_then(|s| s["index"].as_i64())
        };
        let audio = first("audio");
        Ok(Self {
            path,
            duration,
            fps,
            width,
            height,
            audio,
            subtitle: None,
            external_subtitle: None,
            info,
        })
    }
    fn subs(&self, enabled: bool) -> Result<Option<SubSource>> {
        if !enabled {
            return Ok(None);
        }
        if let Some(path) = &self.external_subtitle {
            return Ok(Some(SubSource::Text {
                path: path.clone(),
                stream: 0,
            }));
        }
        let Some(id) = self.subtitle else {
            return Ok(None);
        };
        let streams = self.info["streams"].as_array().unwrap();
        let sub = streams
            .iter()
            .find(|s| s["index"] == id)
            .context("Selected subtitle disappeared")?;
        Ok(Some(
            if ["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle", "xsub"]
                .contains(&sub["codec_name"].as_str().unwrap_or(""))
            {
                SubSource::Bitmap { ff_index: id }
            } else {
                SubSource::Text {
                    path: self.path.clone(),
                    stream: streams
                        .iter()
                        .filter(|s| {
                            s["codec_type"] == "subtitle"
                                && s["index"].as_i64().unwrap_or(i64::MAX) < id
                        })
                        .count(),
                }
            },
        ))
    }
}
struct Job {
    state: Value,
    cancel: Arc<AtomicBool>,
    staging: Option<Scratch>,
    destination: PathBuf,
    staged: PathBuf,
    image: bool,
}
pub struct Engine {
    pub media_revision: u64,
    pub media: Option<Media>,
    pub time: f64,
    pub in_point: Option<f64>,
    pub out_point: Option<f64>,
    pub subtitle_override: Option<Option<SubSource>>,
    pub settings: Settings,
    tool_cache: std::sync::OnceLock<Tools>,
    pub(super) queue_capacity: usize,
    runner: Option<Runner>,
    jobs: BTreeMap<u64, Job>,
    next_id: u64,
}
impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}
impl Engine {
    pub fn new() -> Self {
        Self {
            media_revision: 0,
            subtitle_override: None,
            media: None,
            time: 0.0,
            in_point: None,
            out_point: None,
            settings: Settings::default(),
            tool_cache: std::sync::OnceLock::new(),
            queue_capacity: 32,
            runner: None,
            jobs: BTreeMap::new(),
            next_id: 1,
        }
    }
    pub(super) fn with_tools(tools: Tools) -> Self {
        let engine = Self::new();
        let _ = engine.tool_cache.set(tools);
        engine
    }
    pub(super) fn configure_tools(&mut self, tools: Tools) -> Result<()> {
        if self
            .tool_cache
            .get()
            .is_some_and(|old| old.ffmpeg == tools.ffmpeg && old.gifsicle == tools.gifsicle)
        {
            return Ok(());
        }
        ensure!(
            !self.jobs.values().any(|j| j.staging.is_some()),
            "Tool paths changed while automation jobs are active; wait for them to finish before submitting with the new tools"
        );
        if let Some(runner) = self.runner.take() {
            runner.shutdown();
        }
        self.tool_cache.take();
        let _ = self.tool_cache.set(tools);
        Ok(())
    }
    pub(super) fn tools(&self) -> Tools {
        self.tool_cache
            .get_or_init(|| Tools {
                ffmpeg: self
                    .settings
                    .ffmpeg_path
                    .clone()
                    .unwrap_or_else(|| export::find_tool("ffmpeg")),
                gifsicle: self.settings.gifsicle_path.clone().or_else(|| {
                    let p = export::find_tool("gifsicle");
                    export::tool_version(&p, "--version").ok().map(|_| p)
                }),
            })
            .clone()
    }
    fn runner(&mut self) -> &Runner {
        if self.runner.is_none() {
            self.runner = Some(Runner::headless(self.tools()));
        }
        self.runner.as_ref().unwrap()
    }
    pub fn open(&mut self, path: PathBuf) -> Result<()> {
        let media = Media::probe(path)?;
        self.media_revision = self.media_revision.wrapping_add(1);
        self.media = Some(media);
        self.time = 0.0;
        self.in_point = None;
        self.out_point = None;
        Ok(())
    }
    pub fn state(&self) -> Value {
        json!({"output_base_directory":std::env::current_dir().ok(),"media":self.media.as_ref().map(|m|&m.path),"position_seconds":self.time,"duration_seconds":self.media.as_ref().map(|m|m.duration),"fps":self.media.as_ref().map(|m|m.fps),"in_seconds":self.in_point,"out_seconds":self.out_point,"paused":true})
    }
    fn active(&self) -> Result<&Media> {
        self.media.as_ref().context("Open media first")
    }
    fn protect(&self, args: &Value) -> Result<()> {
        ensure!(
            (self.in_point.is_none() && self.out_point.is_none())
                || args["replace_selection"] == true,
            "Existing selection would be replaced; pass replace_selection=true"
        );
        Ok(())
    }
    pub fn seek_time(&self, args: &Value) -> Result<f64> {
        let m = self.active()?;
        ensure!(
            !(args.get("seconds").is_some() && args.get("frame").is_some()),
            "Specify seconds or frame, not both"
        );
        let time = args["seconds"]
            .as_f64()
            .or_else(|| args["frame"].as_u64().map(|f| f as f64 / m.fps))
            .context("Specify seconds or frame")?;
        position(time, m.duration)
    }
    pub(super) fn is_image_job(&self, id: u64) -> bool {
        self.jobs.get(&id).is_some_and(|j| j.image)
    }
    pub fn job_snapshots(&self) -> Vec<(u64, Value, Arc<AtomicBool>)> {
        self.jobs
            .iter()
            .map(|(id, j)| (*id, j.state.clone(), j.cancel.clone()))
            .collect()
    }
    pub fn poll(&mut self) {
        let Some(events) = self.runner.as_ref().map(|r| r.events.clone()) else {
            return;
        };
        while let Ok(event) = events.try_recv() {
            match event {
                JobEvent::Progress {
                    id, overall, stage, ..
                } => {
                    if let Some(job) = self.jobs.get_mut(&id) {
                        job.state["status"] = json!("running");
                        job.state["progress"] = json!(overall);
                        job.state["stage"] = json!(stage);
                    }
                }
                JobEvent::Finished { id, result } => self.finish(id, result.map(|_| ())),
                JobEvent::Stitched { id, result } => {
                    if let Some(job) = self.jobs.get(&id)
                        && job.destination.extension().is_some_and(|e| e == "psd")
                        && let Ok(out) = &result
                    {
                        self.runner.as_ref().unwrap().submit_psd(
                            id,
                            out.recipe.clone(),
                            out.included.clone(),
                            job.staged.clone(),
                            job.cancel.clone(),
                        );
                        continue;
                    }
                    let result = result.and_then(|out| {
                        let job = self.jobs.get(&id).unwrap();
                        std::fs::copy(&out.artifact.0, &job.staged)
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    });
                    self.finish(id, result);
                }
            }
        }
    }
    fn finish(&mut self, id: u64, result: Result<(), String>) {
        let Some(job) = self.jobs.get_mut(&id) else {
            return;
        };
        let result = result.and_then(|()| {
            if job.cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            // Atomic create-only publication on the destination filesystem. Never rename over a target.
            std::fs::hard_link(&job.staged, &job.destination)
                .map_err(|e| format!("Could not publish output (choose a new filename): {e}"))
        });
        match result {
            Ok(()) => {
                job.state["status"] = json!("done");
                job.state["progress"] = json!(1.0);
                job.state["output"] = json!(job.destination);
            }
            Err(e) => {
                job.state["status"] = json!(if job.cancel.load(Ordering::Relaxed) {
                    "cancelled"
                } else {
                    "failed"
                });
                job.state["error"] = json!(e);
            }
        }
        job.staging = None;
    }
    fn reserve(
        &mut self,
        output: &str,
        image: bool,
        base: Option<&str>,
    ) -> Result<(u64, PathBuf, Arc<AtomicBool>)> {
        ensure!(
            self.jobs.len() < 1000,
            "Session job limit reached; start a new session"
        );
        ensure!(
            self.jobs.values().filter(|j| j.staging.is_some()).count() < self.queue_capacity,
            "Export queue full (capacity {} active/queued jobs); poll jobs_list or cancel a job before retrying",
            self.queue_capacity
        );
        let (destination, base) = output_path(output, base)?;
        ensure!(
            !self
                .jobs
                .values()
                .any(|j| j.destination == destination && j.staging.is_some()),
            "Output already belongs to a queued job"
        );
        let id = self.next_id;
        self.next_id += 1;
        let staging = Scratch::new(destination.parent().unwrap(), id)?;
        let staged = staging.path.join("output").with_extension(
            destination
                .extension()
                .context("Output needs a supported extension")?,
        );
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.insert(id,Job {state:json!({"job_id":id,"status":"queued","progress":0.0,"destination":destination,"base_directory":base}),cancel:cancel.clone(),staging:Some(staging),destination,staged:staged.clone(),image});
        Ok((id, staged, cancel))
    }
    fn reserve_outputs(
        &mut self,
        output: &str,
        both: bool,
        image: bool,
        base: Option<&str>,
    ) -> Result<Vec<(u64, PathBuf, Arc<AtomicBool>)>> {
        let mut paths = vec![PathBuf::from(output)];
        if both {
            let path = Path::new(output);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("Output needs a filename")?;
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .context("Output needs an extension")?;
            paths.push(path.with_file_name(format!("{stem}_raw.{ext}")));
        }
        let mut reserved = Vec::new();
        for path in paths {
            match self.reserve(
                path.to_str().context("Output is not valid UTF-8")?,
                image,
                base,
            ) {
                Ok(job) => reserved.push(job),
                Err(error) => {
                    for (id, _, _) in reserved {
                        self.jobs.remove(&id);
                    }
                    return Err(error);
                }
            }
        }
        Ok(reserved)
    }
    fn submission_result(&self, ids: &[u64], warnings: Vec<&str>) -> Value {
        json!({"job_id":ids[0],"job_ids":ids,"output":self.jobs[&ids[0]].state["destination"],"outputs":ids.iter().map(|id|self.jobs[id].state["destination"].clone()).collect::<Vec<_>>(),"base_directory":self.jobs[&ids[0]].state["base_directory"],"warnings":warnings})
    }
    pub fn call(&mut self, request: &Request) -> Result<Value> {
        super::tools::validate(request)?;
        self.poll();
        let a = &request.arguments;
        match request.name.as_str() {
            "report_issue" => super::reports::accept(a),
            name if name.starts_with("gui_") => bail!(
                "GUI unavailable; launch weebshot --mcp and enable Settings → Advanced → MCP in a running GUI"
            ),
            "media_open" => {
                self.protect(a)?;
                self.open(input_path(a["path"].as_str().unwrap())?)?;
                Ok(self.state())
            }
            "media_close" => {
                self.protect(a)?;
                self.media_revision = self.media_revision.wrapping_add(1);
                self.media = None;
                self.time = 0.0;
                self.in_point = None;
                self.out_point = None;
                Ok(self.state())
            }
            "media_info" => {
                let m = self.active()?;
                let mut info = m.info.clone();
                info["tracks"] = super::identifiers::headless_tracks(
                    &m.info,
                    m.audio,
                    m.subtitle,
                    m.external_subtitle.as_deref(),
                );
                info["track_id_namespace"] = json!("ffprobe");
                Ok(info)
            }
            "media_tracks" => {
                let m = self.active()?;
                Ok(
                    json!({"tracks":super::identifiers::headless_tracks(&m.info,m.audio,m.subtitle,m.external_subtitle.as_deref()),"audio":m.audio,"subtitle":m.subtitle,"track_id_namespace":"ffprobe"}),
                )
            }
            "media_select_track" => {
                let kind = a["kind"].as_str().unwrap();
                ensure!(
                    a.get("track_id").is_some() != a.get("external_path").is_some(),
                    "Specify exactly one of track_id (null disables) or external_path"
                );
                self.active()?;
                if let Some(path) = a["external_path"].as_str() {
                    ensure!(
                        kind == "subtitle",
                        "external_path is only supported for subtitles"
                    );
                    let path = input_path(path)?;
                    ensure!(
                        matches!(
                            path.extension()
                                .and_then(|s| s.to_str())
                                .map(str::to_ascii_lowercase)
                                .as_deref(),
                            Some("ass" | "ssa" | "srt" | "vtt")
                        ),
                        "Headless external subtitles support ASS/SSA/SRT/VTT; external bitmap subtitles are unsupported"
                    );
                    let m = self.media.as_mut().unwrap();
                    m.external_subtitle = Some(path.clone());
                    m.subtitle = None;
                    return Ok(
                        json!({"kind":"subtitle","track_id":null,"id_namespace":"external","external":path}),
                    );
                }
                let id = a["track_id"].as_i64();
                ensure!(
                    a["track_id"].is_null() || id.is_some(),
                    "track_id must fit in a signed 64-bit integer"
                );
                let m = self.active()?;
                let id = super::identifiers::headless_id(
                    &m.info,
                    kind,
                    id,
                    a["id_namespace"].as_str().unwrap_or("native"),
                )?;
                let m = self.media.as_mut().unwrap();
                if kind == "audio" {
                    m.audio = id;
                } else {
                    m.subtitle = id;
                    m.external_subtitle = None;
                }
                Ok(
                    json!({"kind":kind,"track_id":id,"ffprobe_index":id,"mpv_id":null,"id_namespace":"ffprobe"}),
                )
            }
            "timeline_get_state" => Ok(self.state()),
            "timeline_seek" => {
                self.time = self.seek_time(a)?;
                Ok(self.state())
            }
            "timeline_set_in" | "timeline_set_out" | "timeline_set_range" => {
                self.protect(a)?;
                let m = self.active()?;
                let start = if request.name == "timeline_set_in" {
                    Some(a["seconds"].as_f64().unwrap_or(self.time))
                } else {
                    a["in_seconds"].as_f64().or(self.in_point)
                };
                let end = if request.name == "timeline_set_out" {
                    Some(a["seconds"].as_f64().unwrap_or(self.time))
                } else {
                    a["out_seconds"].as_f64().or(self.out_point)
                };
                if let Some(t) = start {
                    position(t, m.duration)?;
                }
                if let Some(t) = end {
                    position(t, m.duration)?;
                }
                if let (Some(s), Some(e)) = (start, end) {
                    range(s, e, m.duration)?;
                }
                self.in_point = start;
                self.out_point = end;
                if a["seek_to_in"] == true {
                    self.time = start.unwrap_or(self.time);
                }
                Ok(self.state())
            }
            "playback_control" => {
                let m = self.active()?;
                match a["action"].as_str().unwrap() {
                    "play" => bail!(
                        "Playback requires an attached GUI; use timeline_seek for headless extraction"
                    ),
                    "step" => {
                        self.time = position(
                            self.time + a["frames"].as_i64().unwrap_or(1) as f64 / m.fps,
                            m.duration,
                        )?
                    }
                    _ => {}
                }
                Ok(self.state())
            }
            "export_get_settings" => Ok(export_settings(&self.settings)),
            "export_set_settings" => {
                self.settings = update_settings(&self.settings, &a["settings"])?;
                Ok(export_settings(&self.settings))
            }
            "screenshot_extract" => self.screenshot(a),
            "export_add" => self.export(a),
            "stitch_create" => self.stitch(a),
            "jobs_list" => Ok(
                json!({"jobs":self.jobs.values().map(|j|j.state.clone()).collect::<Vec<_>>(),"queue_capacity":self.queue_capacity}),
            ),
            "jobs_get" | "jobs_cancel" => {
                let id = a["job_id"].as_u64().unwrap();
                let job = self.jobs.get_mut(&id).context("Unknown job_id")?;
                if request.name == "jobs_cancel" {
                    ensure!(job.staging.is_some(), "Job has already completed");
                    job.cancel.store(true, Ordering::Relaxed);
                    job.state["cancellation_requested"] = json!(true);
                }
                let result = job.state.clone();
                if a["preview"] == true {
                    return super::service::preview(result, job.image);
                }
                Ok(result)
            }
            _ => bail!("Unknown tool"),
        }
    }
    fn crop(&self, a: &Value) -> Result<Option<Crop>> {
        let Some(c) = a.get("crop") else {
            return Ok(None);
        };
        let m = self.active()?;
        let n = |key| c[key].as_u64().unwrap();
        ensure!(
            n("x")
                .checked_add(n("w"))
                .is_some_and(|n| n <= u64::from(m.width))
                && n("y")
                    .checked_add(n("h"))
                    .is_some_and(|n| n <= u64::from(m.height)),
            "Crop exceeds source dimensions"
        );
        Ok(Some(Crop {
            x: n("x") as u32,
            y: n("y") as u32,
            width: n("w") as u32,
            height: n("h") as u32,
        }))
    }
    fn screenshot(&mut self, a: &Value) -> Result<Value> {
        let m = self.active()?;
        let time = if a.get("seconds").is_some() || a.get("frame").is_some() {
            self.seek_time(a)?
        } else {
            self.time
        };
        let output = a["output"].as_str().unwrap();
        ensure!(
            [Some("png"), Some("jpg"), Some("jpeg"), Some("webp")]
                .contains(&Path::new(output).extension().and_then(|s| s.to_str())),
            "Screenshot output extension must be png, jpg, jpeg or webp"
        );
        let job = StillJob {
            input: m.path.clone(),
            time,
            fps: m.fps,
            crop: self.crop(a)?,
            subs: if a["subtitles"]
                .as_bool()
                .unwrap_or(self.settings.shot_subs != export::SubMode::Off)
            {
                match &self.subtitle_override {
                    Some(subs) => subs.clone(),
                    None => m.subs(true)?,
                }
            } else {
                None
            },
            output: PathBuf::new(),
        };
        let warning=(a["subtitles"]==true&&job.subs.is_none()).then_some("No subtitle track is active; no subtitles were rendered. Inspect media_tracks/media_select_track");
        let options = self.settings.shot_anime4k;
        let both = self.settings.shot_subs == export::SubMode::Both
            && a.get("subtitles").is_none()
            && job.subs.is_some();
        let reservations =
            self.reserve_outputs(output, both, true, a["base_directory"].as_str())?;
        let ids = reservations.iter().map(|r| r.0).collect::<Vec<_>>();
        for (n, (id, staged, cancel)) in reservations.into_iter().enumerate() {
            let mut job = job.clone();
            if n > 0 {
                job.subs = None;
            }
            if options.enabled {
                self.runner().submit(
                    id,
                    ExportJob::Image(anime4k::Job {
                        input: job.input.clone(),
                        frame: anime4k::Frame::Extract(job),
                        options,
                        output: Some(staged),
                    }),
                    cancel,
                );
            } else {
                self.runner().submit_still(
                    id,
                    StillJob {
                        output: staged,
                        ..job
                    },
                    cancel,
                );
            }
        }
        Ok(self.submission_result(&ids, warning.into_iter().collect()))
    }
    fn export(&mut self, a: &Value) -> Result<Value> {
        let m = self.active()?;
        let s = if let Some(patch) = a.get("settings") {
            update_settings(&self.settings, patch)?
        } else {
            self.settings.clone()
        };
        let (start, end) = range(
            a["in_seconds"].as_f64().or(self.in_point).unwrap_or(0.0),
            a["out_seconds"]
                .as_f64()
                .or(self.out_point)
                .unwrap_or(m.duration),
            m.duration,
        )?;
        let output = a["output"].as_str().unwrap();
        ensure!(
            Path::new(output).extension().and_then(|s| s.to_str()) == Some(s.clip_format.ext()),
            "Output extension must match clip_format ({})",
            s.clip_format.ext()
        );
        ensure!(
            !s.clip_stream_copy || s.clip_format.supports_stream_copy(),
            "Stream copy supports Mp4 and Mkv only"
        );
        ensure!(
            !s.clip_stream_copy || (!s.clip_anime4k.enabled && a.get("crop").is_none()),
            "Stream copy cannot crop or upscale"
        );
        let job = ClipJob {
            input: m.path.clone(),
            start,
            end,
            format: s.clip_format,
            stream_copy: s.clip_stream_copy,
            quality: s.quality,
            target_mb: (s.use_target_size && s.clip_format.supports_target_size())
                .then_some(s.target_mb),
            audio_ff_index: s.include_audio.then_some(m.audio).flatten(),
            subs: if !s.clip_stream_copy && s.clip_subs != export::SubMode::Off {
                match &self.subtitle_override {
                    Some(subs) => subs.clone(),
                    None => m.subs(true)?,
                }
            } else {
                None
            },
            crop: self.crop(a)?,
            max_height: match s.clip_format {
                export::ClipFormat::Gif => s.anim_height,
                export::ClipFormat::Webp => s.webp_height,
                _ => s.max_height,
            },
            src_height: Some(m.height),
            max_fps: s.max_fps,
            src_fps: Some(m.fps),
            fps: s.anim_fps,
            anim: s.anim,
            encoder: s.encoder,
            loop_count: s.loop_count,
            anime4k: s.clip_anime4k,
            use_gifsicle: self.tools().gifsicle.is_some(),
            output: PathBuf::new(),
        };
        job.target_video_kbps()?;
        let both = s.clip_subs == export::SubMode::Both && job.subs.is_some();
        let reservations =
            self.reserve_outputs(output, both, false, a["base_directory"].as_str())?;
        let ids = reservations.iter().map(|r| r.0).collect::<Vec<_>>();
        for (n, (id, staged, cancel)) in reservations.into_iter().enumerate() {
            let mut job = job.clone();
            job.output = staged;
            if n > 0 {
                job.subs = None;
            }
            self.runner().submit(id, ExportJob::Clip(job), cancel);
        }
        Ok(self.submission_result(&ids, Vec::new()))
    }
    fn stitch(&mut self, a: &Value) -> Result<Value> {
        let m = self.active()?;
        let output = a["output"].as_str().unwrap();
        ensure!(
            matches!(
                Path::new(output).extension().and_then(|s| s.to_str()),
                Some("png" | "psd")
            ),
            "Stitch output must be PNG or PSD"
        );
        let selected = match (
            a["in_seconds"].as_f64().or(self.in_point),
            a["out_seconds"].as_f64().or(self.out_point),
        ) {
            (Some(s), Some(e)) => Some(range(s, e, m.duration)?),
            (None, None) => None,
            _ => bail!("Stitch requires both markers or neither"),
        };
        let mut options = self.settings.stitch.clone();
        if let Some(method) = a.get("method") {
            options.method = serde_json::from_value(method.clone())?;
        }
        if let Some(fps) = a["sampling_fps"].as_f64() {
            options.sampling_fps = fps;
        }
        if let Some(seconds) = a["search_seconds"].as_f64() {
            options.search_seconds = seconds;
        }
        if let Some(divider) = a["layer_divider_percent"].as_u64() {
            options.layer_divider_percent = Some(divider as u32);
        }
        if let Some(prefer) = a["prefer_background"].as_bool() {
            options.prefer_background = prefer;
        }
        let job = crate::stitch::Job {
            input: m.path.clone(),
            playhead: self.time,
            duration: m.duration,
            range: selected,
            source_fps: m.fps,
            options,
        };
        let (id, _, cancel) = self.reserve(
            output,
            Path::new(output).extension().is_some_and(|e| e == "png"),
            a["base_directory"].as_str(),
        )?;
        self.runner().submit_stitch(id, job, cancel);
        Ok(self.submission_result(&[id], Vec::new()))
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(runner) = self.runner.take() {
            runner.shutdown();
        }
        // Staging is discarded on shutdown. Published results remain intact.
    }
}
