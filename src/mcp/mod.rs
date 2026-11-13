//! Optional stdio MCP frontend. Media work is delegated to the existing export worker.
mod engine;
pub mod identifiers;
mod inspection;
pub mod ipc;
mod reports;
pub mod service;
#[cfg(test)]
mod tests;
mod tools;
mod visual;

use anyhow::Result;
pub use engine::{Engine, Media, export_settings, input_path, position, range, update_settings};
use rmcp::{ServerHandler, ServiceExt, model::*, service::RequestContext};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub name: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
}
fn empty_object() -> Value {
    json!({})
}

#[derive(Clone)]
struct Server {
    backend: Arc<Mutex<Backend>>,
    inspections: Arc<visual::Inspections>,
    calls: Arc<tokio::sync::Semaphore>,
    controls: Arc<tokio::sync::Semaphore>,
    previews: Arc<tokio::sync::Semaphore>,
}
enum Backend {
    Headless(Box<Engine>),
    Gui(std::path::PathBuf),
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("weebshot", env!("CARGO_PKG_VERSION")))
            .with_instructions("Local Weebshot automation. Exports return job IDs; poll jobs_get. No output is overwritten. GUI tools require --mcp (without --headless) and an enabled GUI bridge.")
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult {
            tools: tools::definitions(),
            ..Default::default()
        })
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools::definitions().into_iter().find(|t| t.name == name)
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let request = Request {
            name: params.name.to_string(),
            arguments: Value::Object(params.arguments.unwrap_or_default()),
        };
        if inspection::is_tool(&request.name) {
            let result = match tools::validate(&request).and_then(|()| self.inspections.begin()) {
                Ok(ticket) => {
                    let cancel = ticket.cancel.clone();
                    let _cancel_on_drop = visual::CancelOnDrop(cancel.clone());
                    let backend = self.backend.clone();
                    let mut worker =
                        tokio::task::spawn_blocking(move || visual::run(backend, request, ticket));
                    tokio::select! {
                        result=&mut worker => result.map_err(|_|anyhow::anyhow!("Inspection worker stopped")).and_then(|r|r),
                        _=context.ct.cancelled()=> {cancel.store(true,std::sync::atomic::Ordering::Relaxed);worker.await.map_err(|_|anyhow::anyhow!("Inspection worker stopped")).and_then(|r|r)}
                    }
                }
                Err(error) => Err(error),
            };
            return Ok(match result {
                Ok(result) => result,
                Err(error) => CallToolResult::structured_error(
                    json!({"code":"inspection_failed","message":format!("{error:#}")}),
                ),
            }
            .into());
        }
        let is_preview = request.name == "jobs_get" && request.arguments["preview"] == true;
        let slots = if request.name.starts_with("jobs_") && !is_preview {
            self.controls.clone()
        } else {
            self.calls.clone()
        };
        let permit=match slots.try_acquire_owned() {
            Ok(permit)=>permit,
            Err(_)=>return Ok(CallToolResult::structured_error(json!({"code":"busy","message":format!("MCP request capacity reached ({}); wait for pending requests or cancel one",if request.name.starts_with("jobs_")&&!is_preview{8}else{32})})).into()),
        };
        let preview_permit = if is_preview {
            match self.previews.clone().try_acquire_owned(){Ok(p)=>Some(p),Err(_)=>return Ok(CallToolResult::structured_error(json!({"code":"busy","message":"Two image previews are already active; retry later"})).into())}
        } else {
            None
        };
        let backend = self.backend.clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _cancel_on_drop = visual::CancelOnDrop(cancel.clone());
        let worker_cancel = cancel.clone();
        let mut worker = tokio::task::spawn_blocking(move || -> Result<Value> {
            let _permit = permit;
            let _preview_permit = preview_permit;
            tools::validate(&request)?;
            let gui = {
                let backend = backend
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Server state unavailable"))?;
                match &*backend {
                    Backend::Gui(path) => Some(path.clone()),
                    _ => None,
                }
            };
            if let Some(path) = gui {
                return ipc::call_cancellable(&path, &request, &worker_cancel);
            }
            if request.name == "report_issue" {
                return reports::accept(&request.arguments);
            }
            let mut backend = backend
                .lock()
                .map_err(|_| anyhow::anyhow!("Server state unavailable"))?;
            anyhow::ensure!(
                !worker_cancel.load(std::sync::atomic::Ordering::Relaxed),
                "MCP request cancelled before dispatch"
            );
            let Backend::Headless(engine) = &mut *backend else {
                unreachable!()
            };
            if is_preview {
                let mut snapshot = request.clone();
                snapshot.arguments["preview"] = json!(false);
                let result = engine.call(&snapshot)?;
                let image = engine.is_image_job(request.arguments["job_id"].as_u64().unwrap());
                drop(backend);
                return service::preview(result, image);
            }
            engine.call(&request)
        });
        let result = tokio::select! {
            result=&mut worker=>result,
            _=context.ct.cancelled()=> {cancel.store(true,std::sync::atomic::Ordering::Relaxed);worker.await}
        };
        let result = match result {
            Ok(Ok(mut value)) => {
                let preview = value
                    .as_object_mut()
                    .and_then(|v| v.remove("preview_base64"));
                let mut result = CallToolResult::structured(value);
                if let Some(data) = preview.and_then(|v| v.as_str().map(str::to_owned)) {
                    result.content.push(ContentBlock::image(data, "image/png"));
                }
                result
            }
            Ok(Err(error)) => {
                let message = format!("{error:#}");
                let lower = message.to_ascii_lowercase();
                let busy = lower.contains("queue full")
                    || lower.contains("bridge busy")
                    || lower.contains("capacity reached");
                CallToolResult::structured_error(
                    json!({"code":if busy{"busy"}else{"operation_failed"},"message":message}),
                )
            }
            Err(_) => CallToolResult::structured_error(
                json!({"code":"internal_error", "message":"Tool worker stopped"}),
            ),
        };
        Ok(result.into())
    }
}

pub fn run(headless: bool, socket: Option<std::path::PathBuf>) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(async move {
        let backend = if headless {
            Backend::Headless(Box::default())
        } else {
            Backend::Gui(socket.unwrap_or(ipc::default_path()?))
        };
        let inspections = Arc::new(visual::Inspections::default());
        struct ShutdownInspections(Arc<visual::Inspections>);
        impl Drop for ShutdownInspections {
            fn drop(&mut self) {
                self.0.shutdown();
            }
        }
        // Cover early service/transport errors as well as EOF and signal shutdown.
        let _inspection_shutdown = ShutdownInspections(inspections.clone());
        let server = Server {
            inspections: inspections.clone(),
            calls: Arc::new(tokio::sync::Semaphore::new(32)),
            controls: Arc::new(tokio::sync::Semaphore::new(8)),
            previews: Arc::new(tokio::sync::Semaphore::new(2)),
            backend: Arc::new(Mutex::new(backend)),
        };
        // Publish completed outputs even when the client is idle between requests.
        let weak = Arc::downgrade(&server.backend);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(backend) = weak.upgrade() {
                    if let Ok(mut backend) = backend.lock()
                        && let Backend::Headless(engine) = &mut *backend
                    {
                        engine.poll();
                    }
                } else {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        struct Poller(
            Arc<std::sync::atomic::AtomicBool>,
            Option<std::thread::JoinHandle<()>>,
        );
        impl Drop for Poller {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(worker) = self.1.take() {
                    let _ = worker.join();
                }
            }
        }
        let _poller = Poller(stop, Some(worker));
        let service = server.serve(rmcp::transport::stdio_pipes()).await?;
        let cancellation = service.cancellation_token();
        let waiting = service.waiting();
        tokio::pin!(waiting);
        tokio::select! {
            result = &mut waiting => { result?; },
            _ = shutdown_signal() => { cancellation.cancel(); waiting.await?; },
        }
        // Handler cancellation is cooperative: join all decoder/monitor threads
        // before dropping the runtime, including requests interrupted by EOF.
        tokio::task::spawn_blocking(move || inspections.shutdown()).await?;
        Ok(())
    });
    // Tokio's stdio reader uses an uncancellable blocking stdin read. Do not wait
    // for the client to close stdin after SIGTERM; media workers have been joined.
    crate::log::flush_events();
    runtime.shutdown_background();
    result
}

/// Validate new GUI media away from the UI thread, before changing user state.
pub fn preflight(request: &Request, cancel: &std::sync::atomic::AtomicBool) -> Result<()> {
    let a = &request.arguments;
    let path = match request.name.as_str() {
        "media_open" | "gui_open_media" => a["path"].as_str(),
        "gui_prepare_export" => a["media"].as_str(),
        _ => None,
    };
    if let Some(path) = path {
        let media = Media::probe_with_cancel(input_path(path)?, cancel)?;
        if request.name == "gui_prepare_export" {
            range(
                a["in_seconds"].as_f64().unwrap(),
                a["out_seconds"].as_f64().unwrap(),
                media.duration,
            )?;
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = signal.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
