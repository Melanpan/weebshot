//! Private local IPC with bounded connections, FIFO GUI commands and independent polling.
use super::Request;
use anyhow::Result;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

#[cfg(unix)]
pub const COMMAND_CAPACITY: usize = 16;
#[cfg(unix)]
pub const CONNECTION_CAPACITY: usize = 64;
pub struct Pending {
    pub request: Request,
    pub reply: crossbeam_channel::Sender<Result<Value, String>>,
    pub deadline: Instant,
    pub cancel: Arc<AtomicBool>,
}
impl Pending {
    pub fn valid(&self) -> bool {
        Instant::now() < self.deadline && !self.cancel.load(Ordering::Relaxed)
    }
}
#[cfg(unix)]
mod platform {
    use super::super::service::Service;
    use super::*;
    use anyhow::{Context, bail, ensure};
    use serde_json::json;
    use std::{
        fs::{File, OpenOptions},
        io::{ErrorKind, Read, Write},
        os::unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        thread,
        time::Duration,
    };
    const LIMIT: usize = 1024 * 1024;
    const RESPONSE_LIMIT: usize = 8 * LIMIT;
    fn uid() -> Result<u32> {
        Ok(std::fs::metadata(std::env::var_os("HOME").context("HOME unavailable")?)?.uid())
    }
    pub fn default_path() -> Result<PathBuf> {
        Ok(std::env::temp_dir()
            .join(format!("weebshot-mcp-{}", uid()?))
            .join("gui.sock"))
    }
    fn private_parent(path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .context("Socket must have a private parent directory")?;
        match std::fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let meta = std::fs::symlink_metadata(parent)?;
        ensure!(
            meta.is_dir()
                && !meta.file_type().is_symlink()
                && meta.uid() == uid()?
                && meta.permissions().mode() & 0o077 == 0,
            "Socket parent must be a private directory owned by the current user (mode 0700)"
        );
        Ok(())
    }

    fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
        (meta.dev(), meta.ino())
    }

    // Keep this advisory lock for the entire bridge lifetime. The lock file is
    // deliberately persistent: unlinking it would let new instances lock a
    // different inode and race an owner still holding the old one.
    fn ownership_lock(path: &Path) -> Result<File> {
        let mut name = path.as_os_str().to_os_string();
        name.push(".lock");
        let lock_path = PathBuf::from(name);
        let lock = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock_path)
        {
            Ok(lock) => lock,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let meta = std::fs::symlink_metadata(&lock_path)?;
                ensure!(
                    meta.file_type().is_file()
                        && meta.uid() == uid()?
                        && meta.permissions().mode() & 0o077 == 0,
                    "Bridge lock must be a private regular file owned by the current user"
                );
                File::open(&lock_path)?
            }
            Err(error) => return Err(error.into()),
        };
        let disk = std::fs::symlink_metadata(&lock_path)?;
        let opened = lock.metadata()?;
        ensure!(
            disk.file_type().is_file()
                && identity(&disk) == identity(&opened)
                && disk.uid() == uid()?
                && disk.permissions().mode() & 0o077 == 0,
            "Bridge lock must be a private regular file owned by the current user"
        );
        lock.try_lock().with_context(|| {
            format!(
                "Another Weebshot GUI owns the MCP bridge: {}",
                path.display()
            )
        })?;
        Ok(lock)
    }

    struct SocketLease {
        path: PathBuf,
        identity: (u64, u64),
        _lock: File,
    }
    impl Drop for SocketLease {
        fn drop(&mut self) {
            // Never remove a path that has been replaced since we bound it.
            if std::fs::symlink_metadata(&self.path)
                .is_ok_and(|meta| identity(&meta) == self.identity)
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    fn bind_owned(path: &Path) -> Result<(UnixListener, SocketLease)> {
        private_parent(path)?;
        let lock = ownership_lock(path)?;
        let listener = match UnixListener::bind(path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                let previous = std::fs::symlink_metadata(path)?;
                ensure!(
                    previous.file_type().is_socket() && previous.uid() == uid()?,
                    "MCP socket path is occupied by something other than a user-owned socket: {}",
                    path.display()
                );
                // Legacy versions don't take the ownership lock. Probe them
                // without risking a GUI hang on a full accept backlog. This
                // single startup thread is always joined; it never runs jobs.
                let probe_path = path.to_path_buf();
                let probe = thread::spawn(move || -> Result<bool> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_io()
                        .enable_time()
                        .build()?;
                    runtime.block_on(async {
                        match tokio::time::timeout(
                            Duration::from_millis(200),
                            tokio::net::UnixStream::connect(probe_path),
                        )
                        .await
                        {
                            Ok(Err(error)) if error.kind() == ErrorKind::ConnectionRefused => {
                                Ok(true)
                            }
                            _ => Ok(false), // Connected, uncertain, inaccessible or busy: preserve it.
                        }
                    })
                })
                .join()
                .map_err(|_| anyhow::anyhow!("Socket ownership probe failed"))??;
                ensure!(
                    probe,
                    "MCP socket is active or cannot safely be reclaimed: {}. Close the owning GUI or choose another socket",
                    path.display()
                );
                ensure!(
                    identity(&std::fs::symlink_metadata(path)?) == identity(&previous),
                    "MCP socket changed during recovery; retry enabling integration"
                );
                std::fs::remove_file(path)?;
                let listener = UnixListener::bind(path)
                    .context("Cannot bind MCP socket after stale-socket recovery")?;
                crate::log::info("mcp.gui_bridge", "Recovered an abandoned local MCP socket");
                listener
            }
            Err(error) => return Err(error).context("Cannot bind GUI MCP socket"),
        };
        let lease = SocketLease {
            path: path.to_path_buf(),
            identity: identity(&std::fs::symlink_metadata(path)?),
            _lock: lock,
        };
        Ok((listener, lease))
    }
    struct Aux {
        pending: Pending,
        preview: bool,
    }
    struct Connection {
        stream: UnixStream,
        input: Vec<u8>,
        receiver: Option<crossbeam_channel::Receiver<Result<Value, String>>>,
        output: Option<(Vec<u8>, usize)>,
        deadline: Instant,
        cancel: Arc<AtomicBool>,
        operation: String,
    }
    impl Connection {
        fn new(stream: UnixStream) -> Self {
            Self {
                stream,
                input: Vec::new(),
                receiver: None,
                output: None,
                deadline: Instant::now() + Duration::from_secs(1),
                cancel: Arc::new(AtomicBool::new(false)),
                operation: String::new(),
            }
        }
        fn respond(&mut self, result: Result<Value, String>) {
            let response = match result {
                Ok(v) => json!({"result":v}),
                Err(e) => json!({"error":e}),
            };
            let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
            if bytes.len() > RESPONSE_LIMIT {
                bytes = br#"{"error":"IPC response too large; request a smaller result"}"#.to_vec();
            }
            bytes.push(b'\n');
            self.output = Some((bytes, 0));
            self.receiver = None;
            self.deadline = Instant::now() + Duration::from_secs(1);
        }
        fn tick(
            &mut self,
            tx: &crossbeam_channel::Sender<Pending>,
            aux: &crossbeam_channel::Sender<Aux>,
            jobs: Option<&Service>,
            ctx: &eframe::egui::Context,
        ) -> bool {
            if let Some((bytes, offset)) = &mut self.output {
                if Instant::now() >= self.deadline {
                    return false;
                }
                match self.stream.write(&bytes[*offset..]) {
                    Ok(0) => false,
                    Ok(n) => {
                        *offset += n;
                        *offset < bytes.len()
                    }
                    Err(e) => e.kind() == std::io::ErrorKind::WouldBlock,
                }
            } else if let Some(rx) = &self.receiver {
                let mut byte = [0];
                match self.stream.read(&mut byte) {
                    Ok(_) => {
                        self.cancel.store(true, Ordering::Relaxed);
                        return false;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return false,
                }
                if Instant::now() >= self.deadline {
                    self.cancel.store(true, Ordering::Relaxed);
                    self.respond(Err(format!("{} response deadline expired; pending work cancelled. For submissions, check jobs_list before retrying",self.operation)));
                    return true;
                }
                match rx.try_recv() {
                    Ok(result) => self.respond(result),
                    Err(crossbeam_channel::TryRecvError::Disconnected) => self.respond(Err(
                        "GUI responder disconnected; retry when integration is available".into(),
                    )),
                    Err(_) => {}
                }
                true
            } else {
                if Instant::now() >= self.deadline {
                    self.respond(Err("IPC request read timed out".into()));
                    return true;
                }
                let mut chunk = [0; 4096];
                for _ in 0..16 {
                    match self.stream.read(&mut chunk) {
                        Ok(0) => return false,
                        Ok(n) => {
                            self.input.extend_from_slice(&chunk[..n]);
                            if self.input.len() > LIMIT {
                                self.respond(Err("IPC request exceeds 1 MiB".into()));
                                return true;
                            }
                            if self.input.contains(&b'\n') {
                                break;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => return false,
                    }
                }
                let Some(newline) = self.input.iter().position(|b| *b == b'\n') else {
                    return true;
                };
                let parsed = (|| -> Result<Request> {
                    let request = serde_json::from_slice::<Request>(&self.input[..newline])?;
                    super::super::tools::validate(&request)?;
                    Ok(request)
                })();
                let request = match parsed {
                    Ok(r) => r,
                    Err(e) => {
                        self.respond(Err(format!("{e:#}")));
                        return true;
                    }
                };
                self.input.clear();
                self.operation = request.name.clone();
                let report = request.name == "report_issue";
                let preflight = matches!(
                    request.name.as_str(),
                    "media_open" | "gui_open_media" | "gui_prepare_export"
                );
                let submission = super::super::service::is_submission(&request.name);
                self.deadline = Instant::now()
                    + Duration::from_secs(if preflight {
                        15
                    } else if submission {
                        10
                    } else {
                        3
                    });
                let (reply, receiver) = crossbeam_channel::bounded(1);
                let preview = request.name == "jobs_get" && request.arguments["preview"] == true;
                if let Some(jobs) = jobs
                    && matches!(
                        request.name.as_str(),
                        "jobs_list" | "jobs_get" | "jobs_cancel"
                    )
                    && !preview
                {
                    self.respond(jobs.query(&request).map_err(|e| format!("{e:#}")));
                    return true;
                }
                let pending = Pending {
                    request,
                    reply,
                    deadline: self.deadline,
                    cancel: self.cancel.clone(),
                };
                if preflight || report || (preview && jobs.is_some()) {
                    if let Err(e) = aux.try_send(Aux { pending, preview }) {
                        self.respond(Err(format!(
                            "Bridge auxiliary queue full (capacity 16); retry later: {}",
                            e
                        )));
                        return true;
                    }
                } else if let Err(e) = tx.try_send(pending) {
                    self.respond(Err(format!(
                        "GUI command queue full (capacity {COMMAND_CAPACITY}); retry later: {e}"
                    )));
                    return true;
                } else {
                    ctx.request_repaint();
                }
                self.receiver = Some(receiver);
                true
            }
        }
    }
    impl Drop for Connection {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
    pub struct Bridge {
        pub requests: crossbeam_channel::Receiver<Pending>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
        _socket: SocketLease,
    }
    impl Bridge {
        #[cfg(test)]
        pub fn start(path: PathBuf, ctx: eframe::egui::Context) -> Result<Self> {
            Self::with_service(path, ctx, None)
        }
        pub fn with_service(
            path: PathBuf,
            ctx: eframe::egui::Context,
            jobs: Option<Arc<Service>>,
        ) -> Result<Self> {
            let (listener, socket) = bind_owned(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            let (tx, requests) = crossbeam_channel::bounded::<Pending>(COMMAND_CAPACITY);
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let worker = thread::spawn(move || {
                let (aux_tx, aux_rx) = crossbeam_channel::bounded::<Aux>(16);
                let workers = (0..2)
                    .map(|_| {
                        let rx = aux_rx.clone();
                        let tx = tx.clone();
                        let ctx = ctx.clone();
                        let jobs = jobs.clone();
                        let stop = stopped.clone();
                        thread::spawn(move || {
                            while !stop.load(Ordering::Relaxed) {
                                let work = match rx.recv_timeout(Duration::from_millis(20)) {
                                    Ok(w) => w,
                                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                                    Err(_) => break,
                                };
                                let pending = work.pending;
                                if !pending.valid() {
                                    let _ = pending
                                        .reply
                                        .send(Err("Request cancelled before dispatch".into()));
                                    continue;
                                }
                                if pending.request.name == "report_issue" {
                                    let result =
                                        super::super::reports::accept(&pending.request.arguments)
                                            .map_err(|e| format!("{e:#}"));
                                    let _ = pending.reply.send(result);
                                    continue;
                                }
                                if work.preview {
                                    let result = jobs
                                        .as_ref()
                                        .unwrap()
                                        .query(&pending.request)
                                        .map_err(|e| format!("{e:#}"));
                                    let _ = pending.reply.send(result);
                                    continue;
                                }
                                if let Err(error) =
                                    super::super::preflight(&pending.request, &pending.cancel)
                                {
                                    let _ = pending.reply.send(Err(format!("{error:#}")));
                                    continue;
                                }
                                if pending.valid() && !stop.load(Ordering::Relaxed) {
                                    if let Err(error) = tx.try_send(pending) {
                                        let _ = error.into_inner().reply.send(Err(
                                            "GUI command queue full (capacity 16); retry later"
                                                .into(),
                                        ));
                                    } else {
                                        ctx.request_repaint();
                                    }
                                }
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                let mut connections = Vec::<Connection>::new();
                while !stopped.load(Ordering::Relaxed) {
                    for _ in 0..32 {
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                let _ = stream.set_nonblocking(true);
                                if connections.len() >= CONNECTION_CAPACITY {
                                    let _=stream.write_all(b"{\"error\":\"GUI bridge busy: 64 connections already active; retry later\"}\n");
                                } else {
                                    connections.push(Connection::new(stream));
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => break,
                        }
                    }
                    connections.retain_mut(|c| {
                        !stopped.load(Ordering::Relaxed)
                            && c.tick(&tx, &aux_tx, jobs.as_deref(), &ctx)
                    });
                    thread::sleep(Duration::from_millis(5));
                }
                drop(connections);
                drop(aux_tx);
                for worker in workers {
                    let _ = worker.join();
                }
            });
            Ok(Self {
                requests,
                stop,
                worker: Some(worker),
                _socket: socket,
            })
        }
    }
    impl Drop for Bridge {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            for p in self.requests.try_iter() {
                p.cancel.store(true, Ordering::Relaxed);
                let _ = p.reply.send(Err("MCP integration disabled".into()));
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    pub fn call(path: &Path, request: &Request) -> Result<Value> {
        call_cancellable(path, request, &AtomicBool::new(false))
    }
    pub fn call_cancellable(path: &Path, request: &Request, cancel: &AtomicBool) -> Result<Value> {
        private_parent(path)?;
        let mut stream = UnixStream::connect(path)
            .context("GUI unavailable; start Weebshot and enable Settings → Advanced → MCP")?;
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        let mut bytes = serde_json::to_vec(request)?;
        ensure!(bytes.len() <= LIMIT, "Request exceeds IPC size limit");
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut data = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            ensure!(
                !cancel.load(Ordering::Relaxed),
                "MCP request cancelled; pending bridge operation disconnected"
            );
            ensure!(
                Instant::now() < deadline,
                "Bridge response deadline expired; reconnect and check jobs_list before retrying"
            );
            match stream.read(&mut chunk) {
                Ok(0) => bail!("GUI disconnected before replying"),
                Ok(n) => {
                    data.extend_from_slice(&chunk[..n]);
                    ensure!(
                        data.len() <= RESPONSE_LIMIT,
                        "GUI returned an oversized response"
                    );
                    if data.ends_with(b"\n") {
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        let value: Value = serde_json::from_slice(&data)?;
        if let Some(error) = value["error"].as_str() {
            bail!("{error}");
        }
        Ok(value["result"].clone())
    }
}
#[cfg(unix)]
pub use platform::*;
#[cfg(not(unix))]
pub fn default_path() -> Result<PathBuf> {
    anyhow::bail!("GUI attachment currently requires Unix local sockets; headless MCP is available")
}
#[cfg(not(unix))]
pub fn call(_: &Path, _: &Request) -> Result<Value> {
    anyhow::bail!("GUI attachment currently requires Unix local sockets")
}
#[cfg(not(unix))]
pub fn call_cancellable(_: &Path, _: &Request, _: &AtomicBool) -> Result<Value> {
    anyhow::bail!("GUI attachment currently requires Unix local sockets")
}
#[cfg(not(unix))]
pub struct Bridge {
    pub requests: crossbeam_channel::Receiver<Pending>,
}
#[cfg(not(unix))]
impl Bridge {
    pub fn with_service(
        _: PathBuf,
        _: eframe::egui::Context,
        _: Option<Arc<super::service::Service>>,
    ) -> Result<Self> {
        anyhow::bail!("GUI attachment currently requires Unix local sockets")
    }
}
