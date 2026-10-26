use std::path::{Path, PathBuf};

/// `83.456` -> `00:01:23.456`
pub fn fmt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Short form for ruler labels: `1:23`, `1:02:03`, or `1:23.5` when `fine`.
pub fn fmt_ruler(t: f64, fine: bool) -> String {
    let t = t.max(0.0);
    let whole = t.floor() as u64;
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    let base = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };
    if fine {
        format!("{base}.{:02}", ((t.fract() * 100.0).round() as u64).min(99))
    } else {
        base
    }
}

/// SMPTE-style timecode `HH:MM:SS:FF` for a frame rate.
pub fn fmt_timecode(t: f64, fps: f64) -> String {
    let fps_round = fps.round().max(1.0) as u64;
    let frame = frame_at(t, fps);
    let secs = frame / fps_round;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        frame % fps_round
    )
}

pub fn frame_at(t: f64, fps: f64) -> u64 {
    (t.max(0.0) * fps).round() as u64
}

/// Parse a user-typed position: `83.5`, `1:23.5`, `1:02:03.250`, `00:01:23:12`
/// (timecode with frames, when `fps` is known) or `#2000` / `f2000` (frame number).
pub fn parse_position(s: &str, fps: f64) -> Option<f64> {
    let s = s.trim();
    if let Some(frame) = s.strip_prefix('#').or_else(|| s.strip_prefix(['f', 'F'])) {
        let n: f64 = frame.trim().parse().ok()?;
        return (fps > 0.0).then(|| n / fps);
    }
    let parts: Vec<&str> = s.split(':').collect();
    let num = |p: &str| p.trim().parse::<f64>().ok().filter(|v| *v >= 0.0);
    match parts.as_slice() {
        [sec] => num(sec),
        [m, sec] => Some(num(m)? * 60.0 + num(sec)?),
        [h, m, sec] => Some(num(h)? * 3600.0 + num(m)? * 60.0 + num(sec)?),
        [h, m, sec, f] if fps > 0.0 => {
            Some(num(h)? * 3600.0 + num(m)? * 60.0 + num(sec)? + num(f)? / fps.round())
        }
        _ => None,
    }
}

/// Filename-safe timestamp: `83.456` -> `01m23s456`, `3723.5` -> `1h02m03s500`
pub fn time_tag(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    let (h, m, s, frac) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s{frac:03}")
    } else {
        format!("{m:02}m{s:02}s{frac:03}")
    }
}

/// Expand a filename template. Unknown `{tokens}` are left as-is; characters
/// Windows doesn't allow in filenames are replaced.
pub fn expand_template(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out.chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `dir/stem.ext`, or `dir/stem (2).ext` etc. if that already exists.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// Open a folder in the system file manager, selecting `file` if given.
pub fn reveal(dir: &Path, file: Option<&Path>) {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("explorer");
        match file {
            Some(f) => cmd.arg(format!("/select,{}", f.display())),
            None => cmd.arg(dir),
        };
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(dir).spawn();
    }
}

pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.2} MB", b / 1024.0 / 1024.0)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_times() {
        assert_eq!(fmt_time(83.456), "00:01:23.456");
        assert_eq!(fmt_time(3723.5), "01:02:03.500");
        assert_eq!(time_tag(83.456), "01m23s456");
        assert_eq!(time_tag(3723.5), "1h02m03s500");
        assert_eq!(fmt_ruler(83.456, false), "1:23");
        assert_eq!(fmt_ruler(83.456, true), "1:23.46");
        assert_eq!(fmt_ruler(3723.0, false), "1:02:03");
    }

    #[test]
    fn timecodes() {
        let fps = 24000.0 / 1001.0;
        assert_eq!(frame_at(0.0, fps), 0);
        assert_eq!(frame_at(1001.0 / 24000.0 * 100.0, fps), 100);
        assert_eq!(fmt_timecode(1.5, 24.0), "00:00:01:12");
    }

    #[test]
    fn parses_positions() {
        assert_eq!(parse_position("83.5", 24.0), Some(83.5));
        assert_eq!(parse_position("1:23.5", 24.0), Some(83.5));
        assert_eq!(parse_position("1:02:03", 24.0), Some(3723.0));
        assert_eq!(parse_position("00:00:01:12", 24.0), Some(1.5));
        assert_eq!(parse_position("#48", 24.0), Some(2.0));
        assert_eq!(parse_position("abc", 24.0), None);
        assert_eq!(parse_position("-3", 24.0), None);
    }

    #[test]
    fn templates() {
        let vars = [
            ("name", "Show - 01".to_string()),
            ("time", "01m23s456".to_string()),
        ];
        assert_eq!(
            expand_template("{name}_{time}", &vars),
            "Show - 01_01m23s456"
        );
        assert_eq!(
            expand_template("{name}: {nope}", &vars),
            "Show - 01_ {nope}"
        );
    }
}
