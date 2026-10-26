//! Extracts subtitle cue timings with ffmpeg so the timeline can show them.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crossbeam_channel::{Receiver, bounded};
use eframe::egui;

use crate::export::hide_console;
use crate::log;

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Where to read the cues from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueSource {
    pub file: PathBuf,
    /// Absolute stream index for embedded tracks; `None` for a standalone subtitle file.
    pub ff_index: Option<i64>,
}

pub struct CueLoader {
    pub source: CueSource,
    rx: Option<Receiver<Vec<Cue>>>,
    pub cues: Vec<Cue>,
}

impl CueLoader {
    pub fn spawn(ffmpeg: PathBuf, source: CueSource, ctx: egui::Context) -> Self {
        let (tx, rx) = bounded(1);
        let src = source.clone();
        thread::spawn(move || {
            let cues = match extract(&ffmpeg, &src) {
                Ok(c) => {
                    log::info("subs", format!("loaded {} subtitle cues", c.len()));
                    c
                }
                Err(e) => {
                    log::warn("subs", format!("could not read subtitle cues: {e}"));
                    Vec::new()
                }
            };
            let _ = tx.send(cues);
            ctx.request_repaint();
        });
        Self {
            source,
            rx: Some(rx),
            cues: Vec::new(),
        }
    }

    pub fn loading(&self) -> bool {
        self.rx.is_some()
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx
            && let Ok(cues) = rx.try_recv()
        {
            self.cues = cues;
            self.rx = None;
        }
    }

    /// The cue under time `t`, if any.
    pub fn at(&self, t: f64) -> Option<&Cue> {
        self.cues.iter().find(|c| c.start <= t && t < c.end)
    }
}

fn extract(ffmpeg: &Path, src: &CueSource) -> Result<Vec<Cue>, String> {
    let map = match src.ff_index {
        Some(i) => format!("0:{i}"),
        None => "0:s:0".into(),
    };
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(&src.file)
        .args(["-map", &map, "-c:s", "ass", "-f", "ass", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(parse_ass(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_ass_time(s: &str) -> Option<f64> {
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Visible text of an ASS event: override tags removed, `\N` as spaces.
fn strip_ass(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '{' => in_tag = true,
            '}' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("\\N", " ")
        .replace("\\n", " ")
        .replace("\\h", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse `Dialogue:` lines. Drawings (`\p1`) and empty lines (typesetting) are skipped,
/// and duplicates (same timing and text on several layers) collapsed.
pub fn parse_ass(ass: &str) -> Vec<Cue> {
    let mut cues: Vec<Cue> = ass
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Dialogue:")?;
            let fields: Vec<&str> = rest.splitn(10, ',').collect();
            if fields.len() < 10 {
                return None;
            }
            let raw = fields[9];
            if raw.contains("\\p1") || raw.contains("\\p2") || raw.contains("\\p4") {
                return None;
            }
            let text = strip_ass(raw);
            if text.is_empty() {
                return None;
            }
            Some(Cue {
                start: parse_ass_time(fields[1])?,
                end: parse_ass_time(fields[2])?,
                text,
            })
        })
        .collect();
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues.dedup();
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dialogue() {
        let ass = "[Events]\n\
            Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
            Dialogue: 0,0:00:22.00,0:00:25.00,Default,,0,0,0,,{\\i1}Second,{\\i0} line\\Nhere\n\
            Dialogue: 0,0:00:20.00,0:00:22.00,Default,,0,0,0,,First\n\
            Dialogue: 1,0:00:20.00,0:00:22.00,Default,,0,0,0,,First\n\
            Dialogue: 0,0:00:01.00,0:00:02.00,Sign,,0,0,0,,{\\p1}m 0 0 l 10 0 10 10\n";
        let cues = parse_ass(ass);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "First");
        assert_eq!(cues[0].start, 20.0);
        assert_eq!(cues[1].text, "Second, line here");
        assert_eq!(cues[1].end, 25.0);
    }
}
