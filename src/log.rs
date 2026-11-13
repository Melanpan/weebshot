//! In-memory application log shown in the Logs tab.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const MAX_LINES: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub const ALL: [Self; 4] = [Self::Debug, Self::Info, Self::Warn, Self::Error];

    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Line {
    /// Seconds since startup.
    pub time: f64,
    pub level: Level,
    pub source: &'static str,
    pub text: String,
    /// Native structured fields for diagnostic events (also rendered in text).
    pub fields: Option<serde_json::Value>,
    pub timestamp_unix_ms: u64,
}

struct Log {
    start: Instant,
    lines: Mutex<VecDeque<Line>>,
}

static LOG: OnceLock<Log> = OnceLock::new();
/// Errors logged since the Logs tab was last looked at.
static UNSEEN_ERRORS: AtomicUsize = AtomicUsize::new(0);
/// Bumped on every change to the log, so views can cache their filtered copy.
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

fn log_store() -> &'static Log {
    LOG.get_or_init(|| Log {
        start: Instant::now(),
        lines: Mutex::new(VecDeque::new()),
    })
}

pub fn log(level: Level, source: &'static str, text: impl Into<String>) {
    let store = log_store();
    let text = text.into();
    if cfg!(debug_assertions) {
        eprintln!("[{source}] {}: {text}", level.label());
    }
    if level == Level::Error {
        UNSEEN_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    let mut lines = store.lines.lock().unwrap();
    if lines.len() >= MAX_LINES {
        lines.pop_front();
    }
    lines.push_back(Line {
        time: store.start.elapsed().as_secs_f64(),
        level,
        source,
        text,
        fields: None,
        timestamp_unix_ms: unix_millis(),
    });
    SEQUENCE.fetch_add(1, Ordering::Release);
}

/// Changes whenever a line is added or the log is cleared.
pub fn sequence() -> usize {
    SEQUENCE.load(Ordering::Acquire)
}

pub fn info(source: &'static str, text: impl Into<String>) {
    log(Level::Info, source, text);
}

pub fn warn(source: &'static str, text: impl Into<String>) {
    log(Level::Warn, source, text);
}

pub fn error(source: &'static str, text: impl Into<String>) {
    log(Level::Error, source, text);
}

pub fn with_lines<R>(f: impl FnOnce(&VecDeque<Line>) -> R) -> R {
    f(&log_store().lines.lock().unwrap())
}

pub fn clear() {
    log_store().lines.lock().unwrap().clear();
    UNSEEN_ERRORS.store(0, Ordering::Relaxed);
    SEQUENCE.fetch_add(1, Ordering::Release);
}

pub fn unseen_errors() -> usize {
    UNSEEN_ERRORS.load(Ordering::Relaxed)
}

pub fn mark_seen() {
    UNSEEN_ERRORS.store(0, Ordering::Relaxed);
}

/// Render a command line so it can be pasted into a shell.
pub fn format_command(program: &std::path::Path, args: &[String]) -> String {
    let quote = |s: &str| {
        if s.is_empty() || s.contains([' ', '"', '\'', '[', ']', ';', ',', '&', '|']) {
            format!("\"{}\"", s.replace('"', "\\\""))
        } else {
            s.to_string()
        }
    };
    std::iter::once(quote(&program.to_string_lossy()))
        .chain(args.iter().map(|a| quote(a)))
        .collect::<Vec<_>>()
        .join(" ")
}

enum EventRecord {
    Line(String),
    Flush(crossbeam_channel::Sender<()>),
}
static EVENT_SINK: OnceLock<crossbeam_channel::Sender<EventRecord>> = OnceLock::new();

/// Bound shutdown latency even when an external stderr collector stops reading.
pub fn flush_events() {
    if let Some(sink) = EVENT_SINK.get() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if sink
            .send_timeout(
                EventRecord::Flush(tx),
                std::time::Duration::from_millis(100),
            )
            .is_ok()
        {
            let _ = rx.recv_timeout(std::time::Duration::from_millis(500));
        }
    }
}
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Accept structured events into the same bounded Logs buffer. A single bounded
/// stderr worker keeps reports off GUI/IPC threads and supports JSON collectors.
pub fn structured_event(
    level: Level,
    source: &'static str,
    event: &'static str,
    mut fields: serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<String> {
    use serde_json::json;
    static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let sink = EVENT_SINK.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::bounded::<EventRecord>(32);
        std::thread::spawn(move || {
            use std::io::Write;
            for event in rx {
                match event {
                    EventRecord::Line(line) => {
                        let _ = writeln!(std::io::stderr().lock(), "{line}");
                    }
                    EventRecord::Flush(reply) => {
                        let _ = reply.send(());
                    }
                }
            }
        });
        tx
    });
    let timestamp = unix_millis();
    let id = format!(
        "{}-{}",
        std::process::id(),
        IDS.fetch_add(1, Ordering::Relaxed)
    );
    fields.insert("event".into(), json!(event));
    fields.insert("source".into(), json!(source));
    fields.insert(
        "severity".into(),
        json!(if level == Level::Warn {
            "warning"
        } else {
            level.label()
        }),
    );
    fields.insert("timestamp_unix_ms".into(), json!(timestamp));
    fields.insert("report_id".into(), json!(id));
    let fields = serde_json::Value::Object(fields);
    let text = serde_json::to_string(&fields)?;
    let store = log_store();
    let mut lines = store
        .lines
        .lock()
        .map_err(|_| anyhow::anyhow!("Log buffer unavailable"))?;
    sink.try_send(EventRecord::Line(text.clone()))
        .map_err(|_| anyhow::anyhow!("Diagnostic log queue full (32); retry later"))?;
    if lines.len() >= MAX_LINES {
        lines.pop_front();
    }
    lines.push_back(Line {
        time: store.start.elapsed().as_secs_f64(),
        level,
        source,
        text,
        fields: Some(fields),
        timestamp_unix_ms: timestamp,
    });
    if level == Level::Error {
        UNSEEN_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    SEQUENCE.fetch_add(1, Ordering::Release);
    Ok(id)
}
