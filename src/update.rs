//! Update check and self-update from GitHub Releases.
//!
//! Network access goes through the system's `curl` (bundled with Windows 10+, Linux and macOS),
//! so no HTTP/TLS stack is compiled in.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use crossbeam_channel::{Receiver, bounded};
use eframe::egui;

use crate::export::hide_console;
use crate::log;

/// `owner/name` on GitHub, from Cargo.toml's `repository`.
pub fn repo() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Release asset for this platform, as produced by the release workflow.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(windows) {
        Some("weebshot-windows-x64.zip")
    } else if cfg!(target_os = "linux") {
        Some("weebshot-x86_64.AppImage")
    } else if cfg!(target_os = "macos") {
        Some("weebshot-macos-arm64.zip")
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub notes: String,
    /// Download URL of this platform's asset.
    pub download: Option<String>,
}

/// `"v1.2.10"` → `(1, 2, 10)`; a pre-release suffix (`1.2.0-beta`) sorts before the release.
fn parse_version(v: &str) -> Option<(u64, u64, u64, bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, _)) => (c, true),
        None => (v, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch, !pre))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn curl() -> Command {
    let mut cmd = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args(["-fsSL", "--retry", "2", "-A", "weebshot-updater"])
        .stdin(Stdio::null());
    hide_console(&mut cmd);
    cmd
}

fn parse_release(json: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("release has no tag")?;
    let download = asset_name().and_then(|name| {
        v["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"].as_str() == Some(name))?["browser_download_url"]
            .as_str()
            .map(String::from)
    });
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        notes: v["body"].as_str().unwrap_or_default().to_string(),
        download,
    })
}

fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo());
    let out = curl()
        .args(["-H", "Accept: application/vnd.github+json", &url])
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "GitHub didn't answer ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_release(&String::from_utf8_lossy(&out.stdout))
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing,
    /// The new version is in place; quit so the restart can happen.
    ReadyToRestart,
    Failed(String),
}

pub struct Updater {
    pub state: State,
    rx: Option<Receiver<State>>,
}

impl Updater {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            rx: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx
            && let Ok(state) = rx.try_recv()
        {
            self.state = state;
            self.rx = None;
        }
    }

    fn run(
        &mut self,
        initial: State,
        ctx: &egui::Context,
        job: impl FnOnce() -> State + Send + 'static,
    ) {
        let (tx, rx) = bounded(1);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        self.state = initial;
        self.rx = Some(rx);
    }

    pub fn check(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.run(State::Checking, ctx, || match fetch_latest() {
            Ok(r) if is_newer(&r.version, current_version()) => {
                log::info("update", format!("version {} is available", r.version));
                State::Available(r)
            }
            Ok(_) => State::UpToDate,
            Err(e) => {
                log::warn("update", format!("update check failed: {e}"));
                State::Failed(e)
            }
        });
    }

    pub fn install(&mut self, release: Release, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.run(State::Installing, ctx, move || match install(&release) {
            Ok(()) => State::ReadyToRestart,
            Err(e) => {
                log::error("update", format!("update failed: {e}"));
                State::Failed(e)
            }
        });
    }
}

/// Whether this build can replace itself (otherwise the user downloads manually).
pub fn can_self_update() -> bool {
    cfg!(windows) || (cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some())
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    log::info("update", format!("downloading {url}"));
    let status = curl()
        .arg("-o")
        .arg(to)
        .arg(url)
        .status()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("download failed ({status})"))
    }
}

fn install(release: &Release) -> Result<(), String> {
    let url = release
        .download
        .as_deref()
        .ok_or("this release has no download for your platform")?;
    let work = std::env::temp_dir().join("weebshot-update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    if cfg!(windows) {
        install_windows(url, &work)
    } else if let Some(appimage) = std::env::var_os("APPIMAGE") {
        install_appimage(url, &work, Path::new(&appimage))
    } else {
        Err("self-update only works for the Windows build and the AppImage".into())
    }
}

/// Download the zip, unpack it, and hand over to a small script that waits for us to exit,
/// copies the new files over the installation and starts it again.
fn install_windows(url: &str, work: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("can't find the install folder")?;
    let probe = dir.join(".weebshot-write-test");
    std::fs::write(&probe, b"").map_err(|_| {
        format!(
            "can't write to {} — download the update manually instead",
            dir.display()
        )
    })?;
    let _ = std::fs::remove_file(&probe);

    let zip = work.join("update.zip");
    download(url, &zip)?;
    let unpacked = work.join("files");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let mut tar = Command::new(r"C:\Windows\System32\tar.exe");
    tar.arg("-xf").arg(&zip).arg("-C").arg(&unpacked);
    hide_console(&mut tar);
    let ok = tar.status().map_err(|e| e.to_string())?.success();
    if !ok || !unpacked.join("weebshot.exe").exists() {
        return Err("the downloaded update looks broken".into());
    }

    let script = work.join("apply-update.cmd");
    let body = windows_script(std::process::id(), &unpacked, dir);
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("cmd.exe");
    cmd.arg("/C").arg(&script);
    hide_console(&mut cmd);
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn windows_script(pid: u32, from: &Path, to: &Path) -> String {
    format!(
        "@echo off\r\n\
         :wait\r\n\
         tasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul && (timeout /t 1 /nobreak >nul & goto wait)\r\n\
         robocopy \"{from}\" \"{to}\" /E /NFL /NDL /NJH /NJS /NP >nul\r\n\
         start \"\" \"{exe}\"\r\n",
        from = from.display(),
        to = to.display(),
        exe = to.join("weebshot.exe").display(),
    )
}

/// Replace the running AppImage file (Linux allows that) and start the new one.
#[cfg(unix)]
fn install_appimage(url: &str, work: &Path, appimage: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let new = work.join("weebshot.AppImage");
    download(url, &new)?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    // Same filesystem → atomic rename; otherwise copy next to it first.
    let staged: std::path::PathBuf = appimage.with_extension("new");
    std::fs::copy(&new, &staged).map_err(|e| format!("can't write next to the AppImage: {e}"))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&staged, appimage).map_err(|e| e.to_string())?;
    Command::new(appimage).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(unix))]
fn install_appimage(_: &str, _: &Path, _: &Path) -> Result<(), String> {
    Err("AppImages only exist on Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "1.0.0-beta"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn repo_comes_from_cargo_toml() {
        assert_eq!(repo(), "Melanpan/weebshot");
    }

    #[test]
    fn parses_github_release() {
        let json = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/Melanpan/weebshot/releases/tag/v0.2.0",
            "body":"- new stuff","assets":[
              {"name":"weebshot-windows-x64.zip","browser_download_url":"https://example.com/w.zip"},
              {"name":"weebshot-x86_64.AppImage","browser_download_url":"https://example.com/l.AppImage"},
              {"name":"weebshot-macos-arm64.zip","browser_download_url":"https://example.com/m.zip"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.notes, "- new stuff");
        assert!(r.download.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn windows_script_waits_copies_restarts() {
        let s = windows_script(
            1234,
            Path::new(r"C:\tmp\files"),
            Path::new(r"C:\apps\weebshot"),
        );
        assert!(s.contains("PID eq 1234"));
        assert!(s.contains(r#"robocopy "C:\tmp\files" "C:\apps\weebshot" /E"#));
        assert!(s.contains(r#"start "" "C:\apps\weebshot\weebshot.exe""#));
    }
}
