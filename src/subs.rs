//! Extracts subtitle cues with ffmpeg so the timeline can show them, and writes edited
//! ones back out as ASS so the preview/export can burn them in with the original fonts,
//! colors and positioning intact for every line that wasn't touched.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crossbeam_channel::{Receiver, bounded};
use eframe::egui;

use crate::export::{Scratch, hide_console};
use crate::log;

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    /// Stable event identity, independent of timeline sorting and deletion.
    id: usize,
    text_edited: bool,
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

/// Owned preview file: the private directory is created exclusively, and rewrites
/// use the original handle rather than following a path in the shared temp directory.
/// Fields drop in declaration order, closing the file before removing its directory
/// (required on Windows).
pub struct PreviewSubs {
    file: File,
    path: PathBuf,
    _scratch: Scratch,
}

impl PreviewSubs {
    pub fn new() -> anyhow::Result<Self> {
        Self::new_in(&std::env::temp_dir())
    }

    fn new_in(parent: &Path) -> anyhow::Result<Self> {
        let scratch = Scratch::new(parent, 0)?;
        let path = scratch.path.join("preview.ass");
        let mut options = File::options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok(Self {
            file,
            path,
            _scratch: scratch,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&mut self, cues: &CueLoader) -> io::Result<()> {
        self.file.rewind()?;
        self.file.set_len(0)?;
        cues.write_ass(&mut self.file)?;
        self.file.flush()
    }
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

/// Original event order is separate from the sorted, deduplicated timeline.
#[derive(Default)]
struct AssDocument {
    header: String,
    events: Vec<AssEvent>,
}

enum AssEvent {
    Passthrough(String),
    Dialogue { line: String, original: Cue },
}

pub struct CueLoader {
    pub source: CueSource,
    rx: Option<Receiver<(AssDocument, Vec<Cue>)>>,
    pub cues: Vec<Cue>,
    document: AssDocument,
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
            let (document, cues) = match extract(&ffmpeg, &src) {
                Ok((document, cues)) => {
                    log::info("subs", format!("loaded {} subtitle cues", cues.len()));
                    (document, cues)
                }
                Err(e) => {
                    log::warn("subs", format!("could not read subtitle cues: {e}"));
                    (AssDocument::default(), Vec::new())
                }
            };
            let _ = tx.send((document, cues));
            ctx.request_repaint();
        });
        Self {
            source,
            rx: Some(rx),
            cues: Vec::new(),
            document: AssDocument::default(),
            dirty: false,
        }
    }

    pub fn loading(&self) -> bool {
        self.rx.is_some()
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx
            && let Ok((document, cues)) = rx.try_recv()
        {
            self.document = document;
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
            c.text_edited = true;
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
        self.write_ass(File::create(path)?)
    }

    pub fn write_ass(&self, mut f: impl Write) -> io::Result<()> {
        let header = if self.document.header.is_empty() {
            FALLBACK_HEADER
        } else {
            &self.document.header
        };
        f.write_all(header.as_bytes())?;
        let mut cues = vec![None; self.document.events.len()];
        for cue in &self.cues {
            cues[cue.id] = Some(cue);
        }
        for event in &self.document.events {
            match event {
                AssEvent::Passthrough(line) => writeln!(f, "{line}")?,
                AssEvent::Dialogue { line, original } => {
                    // All duplicate effect layers share the visible cue's identity.
                    // Removing that cue removes its layers too, without touching signs.
                    let Some(cue) = cues[original.id] else {
                        continue;
                    };
                    if cue.start == original.start && cue.end == original.end && !cue.text_edited {
                        writeln!(f, "{line}")?;
                    } else {
                        writeln!(
                            f,
                            "Dialogue:{},{},{},{},{}",
                            original.layer,
                            fmt_ass_time(cue.start),
                            fmt_ass_time(cue.end),
                            original.middle,
                            if cue.text_edited {
                                &cue.raw_text
                            } else {
                                &original.raw_text
                            }
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn extract(ffmpeg: &Path, src: &CueSource) -> Result<(AssDocument, Vec<Cue>), String> {
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

/// Keep every source event in file order, exposing only visible, deduplicated cues
/// to the timeline. Hidden typesetting and other source lines pass through verbatim.
fn parse_ass(ass: &str) -> (AssDocument, Vec<Cue>) {
    let mut document = AssDocument::default();
    let mut started = false;
    let mut cues: Vec<Cue> = Vec::new();
    let mut identities = HashMap::new();
    for line in ass.lines() {
        let Some(rest) = line.strip_prefix("Dialogue:") else {
            if !started {
                document.header.push_str(line);
                document.header.push('\n');
            } else {
                document.events.push(AssEvent::Passthrough(line.into()));
            }
            continue;
        };
        started = true;
        let fields: Vec<&str> = rest.splitn(10, ',').collect();
        let parsed = (|| {
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
                id: document.events.len(),
                text_edited: false,
                start: parse_ass_time(fields[1])?,
                end: parse_ass_time(fields[2])?,
                text,
                layer: fields[0].to_string(),
                middle: fields[3..9].join(","),
                raw_text: raw.to_string(),
            })
        })();
        if let Some(mut cue) = parsed {
            let key = (cue.start.to_bits(), cue.end.to_bits(), cue.text.clone());
            if let Some(&id) = identities.get(&key) {
                cue.id = id;
            } else {
                identities.insert(key, cue.id);
                cues.push(cue.clone());
            }
            document.events.push(AssEvent::Dialogue {
                line: line.into(),
                original: cue,
            });
        } else {
            document.events.push(AssEvent::Passthrough(line.into()));
        }
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    (document, cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loader(ass: &str) -> CueLoader {
        let (document, cues) = parse_ass(ass);
        CueLoader {
            source: CueSource {
                file: PathBuf::new(),
                ff_index: None,
            },
            rx: None,
            document,
            cues,
            dirty: false,
        }
    }

    fn output(loader: &CueLoader) -> String {
        let mut out = Vec::new();
        loader.write_ass(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    // Deliberately unsorted, with same-time lines whose source order matters for
    // collision stacking, nonadjacent duplicate layers, and hidden typesetting.
    const ASS: &str = concat!(
        "[Script Info]\nTitle: test\n\n[Events]\n",
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        "Dialogue: 0,0:00:22.00,0:00:25.00,Default,Speaker,40,0,120,,{\\i1}Second,{\\i0} line\\Nhere\n",
        "Dialogue: 0,0:00:20.00,0:00:22.00,Default,,0,0,0,,First\n",
        "Dialogue: 0,0:00:20.00,0:00:22.00,Default,,0,0,0,,Other\n",
        "Dialogue: 1,0:00:20.00,0:00:22.00,Outline,,1,2,3,,{\\bord4}First\n",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Sign,,0,0,0,,{\\p1}m 0 0 l 10 0 10 10\n",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Sign,,0,0,0,,{\\pos(1,2)}\n",
        "Comment: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Keep this comment\n",
    );

    #[test]
    fn preview_rewrites_and_cleans_up_its_private_directory() {
        let root = Scratch::new(&std::env::temp_dir(), 0).unwrap();
        let mut preview = PreviewSubs::new_in(&root.path).unwrap();
        let path = preview.path().to_path_buf();
        let directory = path.parent().unwrap().to_path_buf();
        let mut cues = loader(ASS);
        preview.write(&cues).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ASS);
        // Rewriting a shorter document must not leave stale bytes at the end.
        while !cues.cues.is_empty() {
            cues.delete(0);
        }
        preview.write(&cues).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), output(&cues));
        assert_eq!(preview.path(), path);
        drop(preview);
        assert!(!directory.exists());
    }

    #[test]
    fn previews_are_isolated_and_private() {
        let root = Scratch::new(&std::env::temp_dir(), 0).unwrap();
        let mut first = PreviewSubs::new_in(&root.path).unwrap();
        let mut second = PreviewSubs::new_in(&root.path).unwrap();
        assert_ne!(first.path().parent(), second.path().parent());
        let first_cues = loader(ASS);
        let mut second_cues = loader(ASS);
        second_cues.set_text(0, "Separate preview".into());
        first.write(&first_cues).unwrap();
        second.write(&second_cues).unwrap();
        assert_eq!(std::fs::read_to_string(first.path()).unwrap(), ASS);
        assert_eq!(
            std::fs::read_to_string(second.path()).unwrap(),
            output(&second_cues)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let directory = std::fs::metadata(first.path().parent().unwrap()).unwrap();
            assert_eq!(directory.permissions().mode() & 0o777, 0o700);
            let file = std::fs::metadata(first.path()).unwrap();
            assert_eq!(file.permissions().mode() & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn preview_does_not_follow_a_legacy_filename_symlink() {
        let root = Scratch::new(&std::env::temp_dir(), 0).unwrap();
        let victim = root.path.join("untouched.txt");
        std::fs::write(&victim, "keep this file").unwrap();
        let legacy = root
            .path
            .join(format!("weebshot-{}-preview.subs.ass", std::process::id()));
        std::os::unix::fs::symlink(&victim, &legacy).unwrap();
        let mut preview = PreviewSubs::new_in(&root.path).unwrap();
        preview.write(&loader(ASS)).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep this file");
        assert_eq!(std::fs::read_to_string(preview.path()).unwrap(), ASS);
    }

    #[test]
    fn parses_visible_dialogue_without_losing_source() {
        let loader = loader(ASS);
        assert_eq!(loader.cues.len(), 3);
        assert_eq!(loader.cues[0].text, "First");
        assert_eq!(loader.cues[0].start, 20.0);
        assert_eq!(loader.cues[1].text, "Other");
        assert_eq!(loader.cues[2].text, "Second, line here");
        assert_eq!(loader.cues[2].end, 25.0);
        assert_eq!(output(&loader), ASS);
    }

    #[test]
    fn editing_one_cue_preserves_all_other_events_and_order() {
        let mut loader = loader(ASS);
        loader.set_text(2, "Edited".into());
        assert!(loader.dirty);
        assert_eq!(
            output(&loader),
            ASS.replace("{\\i1}Second,{\\i0} line\\Nhere", "Edited")
        );
    }

    #[test]
    fn retiming_preserves_overrides_and_updates_duplicate_layers() {
        let mut loader = loader(ASS);
        loader.retime(0, 30.5, 32.5);
        let expected = ASS
            .lines()
            .map(|line| {
                if line.ends_with("First") {
                    line.replace("0:00:20.00,0:00:22.00", "0:00:30.50,0:00:32.50")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(output(&loader), expected);
    }

    #[test]
    fn editing_and_deleting_a_cue_apply_to_its_duplicate_layers() {
        let mut loader = loader(ASS);
        loader.set_text(0, "Changed".into());
        let out = output(&loader);
        assert!(!out.contains("First"));
        assert_eq!(out.matches(",Changed\n").count(), 2);
        assert!(out.contains("Outline,,1,2,3,,Changed"));
        loader.delete(0);
        let expected = ASS
            .lines()
            .filter(|line| !line.ends_with("First"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(output(&loader), expected);
        assert_eq!(loader.cues[0].text, "Other");
    }

    #[test]
    fn deleting_every_visible_cue_keeps_hidden_events() {
        let mut loader = loader(ASS);
        while !loader.cues.is_empty() {
            loader.delete(0);
        }
        let out = output(&loader);
        assert!(out.starts_with("[Script Info]\nTitle: test\n"));
        assert!(out.contains("{\\p1}m 0 0 l 10 0 10 10"));
        assert!(out.contains("{\\pos(1,2)}"));
        assert!(out.contains("Keep this comment"));
        assert!(!out.contains("First"));
        assert!(!out.contains("Second"));
        assert!(!out.contains(",Other\n"));
    }

    #[test]
    fn malformed_and_nonstandard_dialogue_passes_through() {
        let ass = "[Events]\nDialogue: malformed\nDialogue: 0,bad,time,Default,,0,0,0,,Text\n";
        assert_eq!(output(&loader(ass)), ass);
    }
}
