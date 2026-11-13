//! Coordinates inspection outside the backend/UI locks and joins it on shutdown.
use super::{Backend, Media, Request, inspection, ipc};
use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use rmcp::model::{CallToolResult, ContentBlock};
use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

#[derive(Default)]
struct State {
    closed: bool,
    active: Vec<Arc<AtomicBool>>,
}
#[derive(Default)]
pub(super) struct Inspections {
    state: Mutex<State>,
    done: Condvar,
}
pub(super) struct Ticket {
    owner: Arc<Inspections>,
    pub cancel: Arc<AtomicBool>,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        state.active.retain(|c| !Arc::ptr_eq(c, &self.cancel));
        self.owner.done.notify_all();
    }
}
impl Inspections {
    pub fn begin(self: &Arc<Self>) -> Result<Ticket> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, "MCP is shutting down");
        ensure!(
            state.active.len() < 2,
            "Two inspections are already active; wait or cancel one before retrying"
        );
        let cancel = Arc::new(AtomicBool::new(false));
        state.active.push(cancel.clone());
        Ok(Ticket {
            owner: self.clone(),
            cancel,
        })
    }
    pub fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        for token in &state.active {
            token.store(true, Ordering::Relaxed);
        }
        while !state.active.is_empty() {
            state = self.done.wait(state).unwrap();
        }
    }
}
pub(super) struct CancelOnDrop(pub Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Identity {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
fn identity(path: &std::path::Path) -> Result<Identity> {
    let meta = std::fs::metadata(path).context("Media was removed or is no longer accessible")?;
    ensure!(meta.is_file(), "Media is no longer a regular file");
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Identity {
        len: meta.len(),
        modified: meta.modified().ok(),
        #[cfg(unix)]
        device: meta.dev(),
        #[cfg(unix)]
        inode: meta.ino(),
    })
}
#[derive(Clone)]
struct Snapshot {
    path: PathBuf,
    revision: u64,
    ffmpeg: PathBuf,
}
fn snapshot(backend: &Mutex<Backend>) -> Result<Snapshot> {
    let socket = {
        let backend = backend
            .lock()
            .map_err(|_| anyhow::anyhow!("Server state unavailable"))?;
        match &*backend {
            Backend::Headless(engine) => {
                return Ok(Snapshot {
                    path: engine
                        .media
                        .as_ref()
                        .context("Open media first")?
                        .path
                        .clone(),
                    revision: engine.media_revision,
                    ffmpeg: engine
                        .settings
                        .ffmpeg_path
                        .clone()
                        .unwrap_or_else(|| crate::export::find_tool("ffmpeg")),
                });
            }
            Backend::Gui(socket) => socket.clone(),
        }
    };
    // Never hold the server/backend mutex across a GUI round trip.
    let state = ipc::call(
        &socket,
        &Request {
            name: "gui_get_state".into(),
            arguments: serde_json::json!({}),
        },
    )?;
    Ok(Snapshot {
        path: state["media"]
            .as_str()
            .map(PathBuf::from)
            .context("Open media in the GUI first")?,
        revision: state["media_revision"]
            .as_u64()
            .context("GUI does not support media revisions; restart the updated application")?,
        ffmpeg: state["ffmpeg_path"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::export::find_tool("ffmpeg")),
    })
}
fn unchanged(backend: &Mutex<Backend>, source: &Snapshot, id: &Identity) -> Result<()> {
    let now = snapshot(backend)
        .context("Media changed, unloaded or GUI integration disconnected during inspection")?;
    ensure!(
        now.path == source.path && now.revision == source.revision,
        "Media changed or unloaded during inspection; retry using the active media"
    );
    ensure!(
        identity(&source.path)? == *id,
        "Media file changed during inspection; reopen it and retry"
    );
    Ok(())
}
struct Monitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn run(
    backend: Arc<Mutex<Backend>>,
    request: Request,
    ticket: Ticket,
) -> Result<CallToolResult> {
    let cancel = ticket.cancel.clone();
    let _ticket = ticket;
    let source = snapshot(&backend)?;
    let id = identity(&source.path)?;
    let changed = Arc::new(Mutex::new(None));
    let error = changed.clone();
    let monitored = source.clone();
    let monitor_backend = backend.clone();
    let monitor_id = id.clone();
    let monitor_cancel = cancel.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let worker = thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) && !monitor_cancel.load(Ordering::Relaxed) {
            if let Err(e) = unchanged(&monitor_backend, &monitored, &monitor_id) {
                *error.lock().unwrap() = Some(format!("{e:#}"));
                monitor_cancel.store(true, Ordering::Relaxed);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    let monitor = Monitor {
        stop,
        worker: Some(worker),
    };
    let result = (|| {
        let media = Media::probe_with_cancel(
            super::input_path(source.path.to_str().context("Media path is not UTF-8")?)?,
            &cancel,
        )?;
        let video = inspection::Video::from_info(media.path, &media.info)?;
        let limits = inspection::Limits::from_env()?;
        let output = inspection::run(&source.ffmpeg, &video, &request, &cancel, &limits)?;
        unchanged(&backend, &source, &id)?;
        ensure!(!cancel.load(Ordering::Relaxed), "Inspection cancelled");
        let mut result = CallToolResult::structured(output.metadata);
        for image in output.images {
            result.content.push(ContentBlock::text(image.description));
            result.content.push(ContentBlock::image(
                base64::prelude::BASE64_STANDARD.encode(image.data),
                image.mime,
            ));
        }
        ensure!(
            serde_json::to_vec(&result)?.len() <= limits.max_response_bytes,
            "MCP response exceeds the payload limit; request fewer frames or lower resolution"
        );
        Ok(result)
    })();
    drop(monitor);
    if let Some(error) = changed.lock().unwrap().take() {
        anyhow::bail!("{error}");
    }
    result
}
