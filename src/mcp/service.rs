//! One bounded FIFO dispatcher for GUI automation, with an independent status cache.
use super::{Engine, Media, Request, ipc::Pending};
use crate::{
    export::{SubSource, Tools},
    settings::Settings,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub const SUBMISSION_CAPACITY: usize = 32;
pub struct ContextSnapshot {
    pub tools: Tools,
    pub media: Option<Media>,
    pub settings: Settings,
    pub time: f64,
    pub in_point: Option<f64>,
    pub out_point: Option<f64>,
    pub subtitles: Option<Option<SubSource>>,
}
struct Work {
    context: ContextSnapshot,
    pending: Pending,
}
#[derive(Clone)]
struct CachedJob {
    state: Value,
    cancel: Arc<AtomicBool>,
    image: bool,
}
#[derive(Default)]
struct Cache {
    jobs: BTreeMap<u64, CachedJob>,
}
pub struct Service {
    tx: Option<crossbeam_channel::Sender<Work>>,
    cache: Arc<Mutex<Cache>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Service {
    pub fn new(tools: Tools) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<Work>(SUBMISSION_CAPACITY);
        let cache = Arc::new(Mutex::new(Cache::default()));
        let shared = cache.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let mut engine = Engine::with_tools(tools);
            let publish = |engine: &Engine| {
                let jobs = engine
                    .job_snapshots()
                    .into_iter()
                    .map(|(id, mut state, cancel)| {
                        if cancel.load(Ordering::Relaxed) {
                            state["cancellation_requested"] = json!(true);
                        }
                        (
                            id,
                            CachedJob {
                                state,
                                cancel,
                                image: engine.is_image_job(id),
                            },
                        )
                    })
                    .collect();
                shared.lock().unwrap().jobs = jobs;
            };
            while !stopped.load(Ordering::Relaxed) {
                engine.poll();
                publish(&engine);
                let work = match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(work) => work,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let Work { context, pending } = work;
                if !pending.valid() {
                    let _ = pending.reply.send(Err(
                        "Submission cancelled or expired before dispatch; no job submitted".into(),
                    ));
                    continue;
                }
                if let Err(error) = engine.configure_tools(context.tools) {
                    let _ = pending.reply.send(Err(format!("{error:#}")));
                    continue;
                }
                engine.media = context.media;
                engine.settings = context.settings;
                engine.time = context.time;
                engine.in_point = context.in_point;
                engine.out_point = context.out_point;
                engine.subtitle_override = context.subtitles;
                let result = engine.call(&pending.request).map_err(|e| format!("{e:#}"));
                engine.poll();
                publish(&engine);
                let ids = result
                    .as_ref()
                    .ok()
                    .and_then(|v| v["job_ids"].as_array())
                    .map(|a| a.iter().filter_map(Value::as_u64).collect::<Vec<_>>())
                    .unwrap_or_default();
                if !pending.valid() {
                    cancel_ids(&shared, &ids);
                    let _ = pending.reply.send(Err(
                        "Submission cancelled or timed out; any submitted jobs were cancelled"
                            .into(),
                    ));
                } else if pending.reply.send(result).is_err() {
                    cancel_ids(&shared, &ids);
                }
            }
            for work in rx.try_iter() {
                let _ = work
                    .pending
                    .reply
                    .send(Err("MCP integration disabled; submission rejected".into()));
            }
            // Engine owns and joins its media worker; no subprocess survives shutdown.
        });
        Self {
            tx: Some(tx),
            cache,
            stop,
            worker: Some(worker),
        }
    }
    pub fn submit(&self, context: ContextSnapshot, pending: Pending) {
        let work = Work { context, pending };
        if self.stop.load(Ordering::Relaxed) {
            let _ = work
                .pending
                .reply
                .send(Err("MCP integration disabled".into()));
            return;
        }
        if let Err(error) = self.tx.as_ref().unwrap().try_send(work) {
            let _=error.into_inner().pending.reply.send(Err(format!("Submission queue full (capacity {SUBMISSION_CAPACITY}); poll jobs_list and retry later")));
        }
    }
    pub fn snapshots(&self) -> Vec<(u64, Value, Arc<AtomicBool>)> {
        self.cache
            .lock()
            .unwrap()
            .jobs
            .iter()
            .map(|(id, j)| (*id, j.state.clone(), j.cancel.clone()))
            .collect()
    }
    pub fn query(&self, request: &Request) -> Result<Value> {
        ensure!(
            !self.stop.load(Ordering::Relaxed),
            "MCP integration disabled"
        );
        let a = &request.arguments;
        if request.name == "jobs_list" {
            return Ok(
                json!({"jobs":self.snapshots().into_iter().map(|(_,s,_)|s).collect::<Vec<_>>(),"queue_capacity":32,"submission_capacity":SUBMISSION_CAPACITY}),
            );
        }
        let id = a["job_id"].as_u64().context("Provide job_id")?;
        let job = self
            .cache
            .lock()
            .unwrap()
            .jobs
            .get(&id)
            .cloned()
            .context("Unknown job_id; use the job_id returned by submission")?;
        let mut result = job.state;
        if request.name == "jobs_cancel" {
            ensure!(
                matches!(result["status"].as_str(), Some("queued" | "running")),
                "Job has already completed"
            );
            job.cancel.store(true, Ordering::Relaxed);
            result["cancellation_requested"] = json!(true);
        }
        if a["preview"] == true {
            return preview(result, job.image);
        }

        Ok(result)
    }
}
pub(super) fn preview(mut result: Value, image_job: bool) -> Result<Value> {
    if !image_job || result["status"] != "done" {
        return Ok(result);
    }
    let path = result["output"].as_str().context("Job has no output")?;
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str());
    ensure!(
        extension != Some("webp"),
        "WebP preview is unavailable; extract PNG or JPEG for MCP image content"
    );
    ensure!(
        matches!(extension, Some("png" | "jpg" | "jpeg")),
        "Preview requires a completed PNG or JPEG image job"
    );
    use base64::Engine as _;
    let image = image::open(path)?.thumbnail(512, 512);
    let mut data = std::io::Cursor::new(Vec::new());
    image.write_to(&mut data, image::ImageFormat::Png)?;
    result["preview_base64"] = json!(base64::prelude::BASE64_STANDARD.encode(data.into_inner()));
    Ok(result)
}
fn cancel_ids(cache: &Mutex<Cache>, ids: &[u64]) {
    let cache = cache.lock().unwrap();
    for id in ids {
        if let Some(j) = cache.jobs.get(id) {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for (_, _, flag) in self.snapshots() {
            flag.store(true, Ordering::Relaxed);
        }
        drop(self.tx.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
pub fn is_submission(name: &str) -> bool {
    matches!(name, "screenshot_extract" | "export_add" | "stitch_create")
}
