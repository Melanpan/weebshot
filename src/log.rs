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
}

struct Log {
    start: Instant,
    lines: Mutex<VecDeque<Line>>,
}

static LOG: OnceLock<Log> = OnceLock::new();
/// Errors logged since the Logs tab was last looked at.
static UNSEEN_ERRORS: AtomicUsize = AtomicUsize::new(0);

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
    });
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
