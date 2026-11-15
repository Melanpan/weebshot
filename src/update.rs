//! Update check and self-update from GitHub Releases.
//!
//! Network access goes through the system's `curl` (bundled with Windows 10+, Linux and macOS),
//! so no HTTP/TLS stack is compiled in.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use crossbeam_channel::{Receiver, bounded};
use eframe::egui;

use crate::export::{Scratch, hide_console};
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

/// `max_secs` bounds the whole transfer: a stalled connection must not leave the
/// updater "busy" for the rest of the session.
fn curl(max_secs: u32) -> Command {
    let mut cmd = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args(["-fsSL", "--retry", "2", "-A", "weebshot-updater"])
        .args([
            "--connect-timeout",
            "15",
            "--max-time",
            &max_secs.to_string(),
        ])
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
    let out = curl(30)
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
    let status = curl(15 * 60)
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
    let work = update_workspace(&std::env::temp_dir())?;

    if cfg!(windows) {
        install_windows(url, work)
    } else if let Some(appimage) = std::env::var_os("APPIMAGE") {
        install_appimage(url, &work.path, Path::new(&appimage))
    } else {
        Err("self-update only works for the Windows build and the AppImage".into())
    }
}

fn update_workspace(parent: &Path) -> Result<Scratch, String> {
    Scratch::new(parent, 0).map_err(|e| format!("couldn't create update workspace: {e:#}"))
}

/// Download the zip, unpack it, and hand over to a small script that waits for us to exit,
/// copies the new files over the installation and starts it again.
fn install_windows(url: &str, work: Scratch) -> Result<(), String> {
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

    let zip = work.path.join("update.zip");
    download(url, &zip)?;
    let unpacked = work.path.join("files");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let mut tar = Command::new(r"C:\Windows\System32\tar.exe");
    tar.arg("-xf").arg(&zip).arg("-C").arg(&unpacked);
    hide_console(&mut tar);
    let ok = tar.status().map_err(|e| e.to_string())?.success();
    if !ok || !unpacked.join("weebshot.exe").exists() {
        return Err("the downloaded update looks broken".into());
    }

    let script = work.path.join("apply-update.cmd");
    let body = windows_script(std::process::id(), &work.path, dir);
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/D", "/C"]).arg(&script);
    hide_console(&mut cmd);
    cmd.spawn().map_err(|e| e.to_string())?;
    // The helper needs its script and unpacked files after this process exits.
    // Until spawn succeeds, the guard cleans up every failure automatically.
    work.keep();
    Ok(())
}

fn windows_script(pid: u32, work: &Path, to: &Path) -> String {
    format!(
        "@echo off\r\n\
         setlocal DisableDelayedExpansion\r\n\
         chcp 65001 >nul\r\n\
         :wait\r\n\
         tasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul && (timeout /t 1 /nobreak >nul & goto wait)\r\n\
         robocopy \"{from}\" \"{to}\" /E /NFL /NDL /NJH /NJS /NP >nul\r\n\
         if errorlevel 8 (\r\n\
         powershell.exe -NoProfile -Command \"Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('Weebshot could not copy the update. Please download and install it manually.', 'Weebshot update failed') | Out-Null\"\r\n\
         rmdir /S /Q \"{work}\"\r\n\
         exit /b 1\r\n\
         )\r\n\
         start \"\" \"{exe}\"\r\n\
         rmdir /S /Q \"{work}\"\r\n",
        work = work.display(),
        from = work.join("files").display(),
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

    #[test]
    fn windows_script_waits_copies_restarts_and_cleans_up() {
        let work = Path::new("update workspace");
        let to = Path::new("install directory");
        let s = windows_script(1234, work, to);
        let copy = format!(
            r#"robocopy "{}" "{}" /E"#,
            work.join("files").display(),
            to.display()
        );
        let restart = format!(r#"start "" "{}""#, to.join("weebshot.exe").display());
        let cleanup = format!(r#"rmdir /S /Q "{}""#, work.display());
        assert!(s.contains("PID eq 1234"));
        assert!(s.find("chcp 65001").unwrap() < s.find(":wait").unwrap());
        let failure = s.find("if errorlevel 8 (").unwrap();
        assert!(s.find(&copy).unwrap() < failure);
        assert!(failure < s.find("exit /b 1").unwrap());
        assert!(s.find("exit /b 1").unwrap() < s.find(&restart).unwrap());
        let copy_at = s.find(&copy).unwrap();
        let restart_at = s.find(&restart).unwrap();
        let cleanup_at = s.rfind(&cleanup).unwrap();
        assert!(s.find(":wait").unwrap() < copy_at);
        assert!(copy_at < restart_at && restart_at < cleanup_at);
        // The self-deleting cleanup must be the last command in the batch file.
        assert!(s.ends_with(&(cleanup + "\r\n")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_helper_can_delete_its_own_workspace() {
        let root = Scratch::new(&std::env::temp_dir(), 0).unwrap();
        let work = update_workspace(&root.path).unwrap();
        let script = work.path.join("apply-update.cmd");
        let body = windows_script(1234, &work.path, &root.path);
        // Exercise the actual final command without replacing or restarting an app.
        let cleanup = body.lines().last().unwrap();
        std::fs::write(&script, format!("@echo off\r\n{cleanup}\r\n")).unwrap();
        let path = work.keep();
        let output = Command::new("cmd.exe")
            .args(["/D", "/C"])
            .arg(&script)
            .current_dir(&root.path)
            .output()
            .unwrap();
        assert!(
            !path.exists(),
            "helper did not clean up: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn windows_script_selects_utf8_before_non_ascii_paths() {
        let work = Path::new("Users/Zoë/update");
        let to = Path::new("Users/Zoë/Weebshot");
        let script = windows_script(1234, work, to);
        assert!(script.find("chcp 65001").unwrap() < script.find("Zoë").unwrap());
        assert!(script.contains(&work.join("files").display().to_string()));
        assert!(script.contains(&to.join("weebshot.exe").display().to_string()));
    }

    #[test]
    fn update_workspaces_are_isolated_and_cleaned_up() {
        let root = Scratch::new(&std::env::temp_dir(), 0).unwrap();
        let legacy = root.path.join("weebshot-update");
        std::fs::create_dir(&legacy).unwrap();
        let sentinel = legacy.join("unrelated.txt");
        std::fs::write(&sentinel, "keep").unwrap();
        let first = update_workspace(&root.path).unwrap();
        let second = update_workspace(&root.path).unwrap();
        assert_ne!(first.path, second.path);
        let first_path = first.path.clone();
        let second_path = second.path.clone();
        std::fs::write(first.path.join("partial.zip"), "partial download").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&first.path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        drop(first);
        assert!(!first_path.exists());
        assert!(second_path.exists());
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        drop(second);
        assert!(!second_path.exists());
    }

    #[test]
    fn helper_handoff_keeps_workspace_until_explicit_cleanup() {
        let root = Scratch::new(&std::env::temp_dir(), 0).unwrap();
        let work = update_workspace(&root.path).unwrap();
        let script = work.path.join("apply-update.cmd");
        std::fs::write(&script, "helper").unwrap();
        let path = work.keep();
        assert_eq!(std::fs::read_to_string(&script).unwrap(), "helper");
        std::fs::remove_dir_all(&path).unwrap();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn update_workspace_does_not_follow_legacy_symlink() {
        let root = Scratch::new(&std::env::temp_dir(), 0).unwrap();
        let victim = root.path.join("victim");
        std::fs::create_dir(&victim).unwrap();
        let sentinel = victim.join("unrelated.txt");
        std::fs::write(&sentinel, "keep").unwrap();
        std::os::unix::fs::symlink(&victim, root.path.join("weebshot-update")).unwrap();
        let work = update_workspace(&root.path).unwrap();
        std::fs::write(work.path.join("update.zip"), "download").unwrap();
        drop(work);
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        assert!(!victim.join("update.zip").exists());
    }
}
