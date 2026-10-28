//! Extracts subtitle cues with ffmpeg so the timeline can show them, and writes edited
//! ones back out as ASS so the preview/export can burn them in with the original fonts,
//! colors and positioning intact for every line that wasn't touched.

use std::fs::File;
use std::io::{self, Write};
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
    /// Plain display text, shown in the timeline and the inline editor.
    pub text: String,
    /// Original `Layer` field, verbatim.
    layer: String,
    /// Original `Style,Name,MarginL,MarginR,MarginV,Effect` fields, verbatim — kept as-is
    /// even on edited lines, so per-line style, position and margin overrides survive.
    middle: String,
    /// Original raw ASS `Text` field (override tags, `\N`, verbatim) — replaced with
    /// `text` once the line is edited, which loses those for that line only.
    raw_text: String,
}

/// Where to read the cues from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueSource {
    pub file: PathBuf,
    /// Absolute stream index for embedded tracks; `None` for a standalone subtitle file.
    pub ff_index: Option<i64>,
}

/// A minimal valid ASS header, used if the source couldn't be parsed for one.
const FALLBACK_HEADER: &str = "[Script Info]\nScriptType: v4.00+\n\n\
    [V4+ Styles]\n\
    Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
    Style: Default,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H64000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n\
    [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

pub struct CueLoader {
    pub source: CueSource,
    rx: Option<Receiver<(String, Vec<Cue>)>>,
    pub cues: Vec<Cue>,
    /// Everything up to and including the `[Events]` `Format:` line of the source ASS,
    /// reused verbatim so a rewritten file keeps the original fonts/styles.
    header: String,
    /// Set once the user deletes/edits/retimes a cue. While dirty, the preview and export
    /// show these cues directly instead of the original file's native subtitle rendering,
    /// since there's no way to patch just one line of it in place.
    pub dirty: bool,
}

impl CueLoader {
    pub fn spawn(ffmpeg: PathBuf, source: CueSource, ctx: egui::Context) -> Self {
        let (tx, rx) = bounded(1);
        let src = source.clone();
        thread::spawn(move || {
            let (header, cues) = match extract(&ffmpeg, &src) {
                Ok((header, cues)) => {
                    log::info("subs", format!("loaded {} subtitle cues", cues.len()));
                    (header, cues)
                }
                Err(e) => {
                    log::warn("subs", format!("could not read subtitle cues: {e}"));
                    (String::new(), Vec::new())
                }
            };
            let _ = tx.send((header, cues));
            ctx.request_repaint();
        });
        Self {
            source,
            rx: Some(rx),
            cues: Vec::new(),
            header: String::new(),
            dirty: false,
        }
    }

    pub fn loading(&self) -> bool {
        self.rx.is_some()
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx
            && let Ok((header, cues)) = rx.try_recv()
        {
            self.header = header;
            self.cues = cues;
            self.rx = None;
        }
    }

    /// The cue under time `t`, if any.
    pub fn at(&self, t: f64) -> Option<&Cue> {
        self.cues.iter().find(|c| c.start <= t && t < c.end)
    }

    pub fn delete(&mut self, idx: usize) {
        if idx < self.cues.len() {
            self.cues.remove(idx);
            self.dirty = true;
        }
    }

    pub fn set_text(&mut self, idx: usize, text: String) {
        if let Some(c) = self.cues.get_mut(idx) {
            c.raw_text = text.clone();
            c.text = text;
            self.dirty = true;
        }
    }

    pub fn retime(&mut self, idx: usize, start: f64, end: f64) {
        if let Some(c) = self.cues.get_mut(idx) {
            c.start = start;
            c.end = end;
            self.dirty = true;
        }
    }

    /// Write the current cues back out as ASS, reusing the original header so untouched
    /// lines keep their original fonts/colors/position.
    pub fn write_to(&self, path: &Path) -> io::Result<()> {
        let mut f = File::create(path)?;
        let header = if self.header.is_empty() {
            FALLBACK_HEADER
        } else {
            &self.header
        };
        f.write_all(header.as_bytes())?;
        for cue in &self.cues {
            writeln!(
                f,
                "Dialogue: {},{},{},{},{}",
                cue.layer,
                fmt_ass_time(cue.start),
                fmt_ass_time(cue.end),
                cue.middle,
                cue.raw_text
            )?;
        }
        Ok(())
    }
}

fn extract(ffmpeg: &Path, src: &CueSource) -> Result<(String, Vec<Cue>), String> {
    let map = match src.ff_index {
        Some(i) => format!("0:{i}"),
        None => "0:s:0".into(),
    };
    // A stream copy passes the original ASS/SSA header through byte-for-byte, keeping
    // its real style sheet. Only works if the source is already ASS/SSA, so fall back
    // to transcoding for other formats (SRT, mov_text, WebVTT, ...) — those have no
    // style sheet to lose, so ffmpeg synthesizing a generic one is harmless there.
    let out = run_extract(ffmpeg, src, &map, "copy")
        .or_else(|_| run_extract(ffmpeg, src, &map, "ass"))?;
    Ok(parse_ass(&out))
}

fn run_extract(
    ffmpeg: &Path,
    src: &CueSource,
    map: &str,
    sub_codec: &str,
) -> Result<String, String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(&src.file)
        .args(["-map", map, "-c:s", sub_codec, "-f", "ass", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_ass_time(s: &str) -> Option<f64> {
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// `H:MM:SS.CC`, the ASS/SSA time format (centiseconds).
fn fmt_ass_time(t: f64) -> String {
    let t = t.max(0.0);
    let total_cs = (t * 100.0).round() as u64;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
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

/// Parse an ffmpeg-produced ASS script into its header (everything up to and including
/// the `[Events]` `Format:` line — script info and style definitions, reused verbatim to
/// rewrite the file later) and its `Dialogue:` lines as `Cue`s. Drawings (`\p1`) and empty
/// lines (typesetting) are skipped, and duplicates (same timing/text on several layers)
/// collapsed.
fn parse_ass(ass: &str) -> (String, Vec<Cue>) {
    let mut header = String::new();
    let mut started = false;
    let mut cues: Vec<Cue> = Vec::new();
    for line in ass.lines() {
        let Some(rest) = line.strip_prefix("Dialogue:") else {
            if !started {
                header.push_str(line);
                header.push('\n');
            }
            continue;
        };
        started = true;
        let fields: Vec<&str> = rest.splitn(10, ',').collect();
        if fields.len() < 10 {
            continue;
        }
        let raw = fields[9];
        if raw.contains("\\p1") || raw.contains("\\p2") || raw.contains("\\p4") {
            continue;
        }
        let text = strip_ass(raw);
        if text.is_empty() {
            continue;
        }
        let (Some(start), Some(end)) = (parse_ass_time(fields[1]), parse_ass_time(fields[2]))
        else {
            continue;
        };
        cues.push(Cue {
            start,
            end,
            text,
            layer: fields[0].to_string(),
            middle: fields[3..9].join(","),
            raw_text: raw.to_string(),
        });
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    // Collapse by timing+text, not full equality: duplicate layers (e.g. an outline
    // effect) now carry distinct `layer`/`middle` fields but should still count as
    // one line in the timeline.
    cues.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.text == b.text);
    (header, cues)
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
        let (header, cues) = parse_ass(ass);
        assert!(header.contains("[Events]"));
        assert!(header.contains("Format:"));
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "First");
        assert_eq!(cues[0].start, 20.0);
        assert_eq!(cues[1].text, "Second, line here");
        assert_eq!(cues[1].end, 25.0);
    }

    #[test]
    fn loader_mutations() {
        let mut loader = CueLoader {
            source: CueSource {
                file: PathBuf::new(),
                ff_index: None,
            },
            rx: None,
            header: String::new(),
            cues: vec![
                Cue {
                    start: 1.0,
                    end: 2.0,
                    text: "a".into(),
                    layer: "0".into(),
                    middle: "Default,,0,0,0,".into(),
                    raw_text: "a".into(),
                },
                Cue {
                    start: 3.0,
                    end: 4.0,
                    text: "b".into(),
                    layer: "0".into(),
                    middle: "Default,,0,0,0,".into(),
                    raw_text: "b".into(),
                },
            ],
            dirty: false,
        };
        loader.set_text(0, "edited".into());
        assert_eq!(loader.cues[0].text, "edited");
        assert_eq!(loader.cues[0].raw_text, "edited");
        assert!(loader.dirty);
        loader.retime(1, 3.5, 4.5);
        assert_eq!((loader.cues[1].start, loader.cues[1].end), (3.5, 4.5));
        loader.delete(0);
        assert_eq!(loader.cues.len(), 1);
        assert_eq!(loader.cues[0].text, "b");
    }

    #[test]
    fn writes_ass_reusing_header() {
        let loader = CueLoader {
            source: CueSource {
                file: PathBuf::new(),
                ff_index: None,
            },
            rx: None,
            header: "[Script Info]\nTitle: test\n\n[Events]\n\
                Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
                .to_string(),
            cues: vec![Cue {
                start: 1.0,
                end: 2.5,
                text: "Hello".into(),
                layer: "0".into(),
                middle: "Default,Speaker,40,0,120,".into(),
                raw_text: "{\\i1}Hello".into(),
            }],
            dirty: true,
        };
        let path = std::env::temp_dir().join("weebshot-subs-test.ass");
        loader.write_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(out.starts_with("[Script Info]\nTitle: test\n"));
        assert!(
            out.contains(
                "Dialogue: 0,0:00:01.00,0:00:02.50,Default,Speaker,40,0,120,,{\\i1}Hello\n"
            )
        );
    }
}
