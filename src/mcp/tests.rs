use super::*;
use serde_json::json;

#[test]
fn defaults_and_settings_migration_keep_mcp_disabled() {
    let old: crate::settings::Settings = ron::from_str("(clip_format: Mp4)").unwrap();
    assert!(!old.mcp_enabled);
    let mut current = old;
    current.mcp_enabled = true;
    let restored: crate::settings::Settings =
        ron::from_str(&ron::to_string(&current).unwrap()).unwrap();
    assert!(restored.mcp_enabled);
}
#[test]
fn validation_rejects_unknown_fields_types_ranges_and_executable_settings() {
    for (name, args) in [
        ("timeline_seek", json!({"seconds":-1})),
        ("timeline_seek", json!({"seconds":"1"})),
        (
            "export_set_settings",
            json!({"settings":{"ffmpeg_path":"/tmp/program"}}),
        ),
        (
            "export_set_settings",
            json!({"settings":{"encoder":{"x264_preset":"-anything"}}}),
        ),
        (
            "export_add",
            json!({"output":"x.gif","settings":{"anim":{"colors":1000}}}),
        ),
        ("media_select_track", json!({"kind":"audio"})),
        ("jobs_get", json!({"job_id":1,"arbitrary":true})),
        ("missing_tool", json!({})),
    ] {
        assert!(
            tools::validate(&Request {
                name: name.into(),
                arguments: args
            })
            .is_err(),
            "{name}"
        );
    }
    assert!(position(f64::NAN, 3.0).is_err());
    assert!(position(4.0, 3.0).is_err());
    assert!(range(2.0, 1.0, 3.0).is_err());
}
#[test]
fn patch_reuses_presets_and_preserves_unrelated_settings() {
    let settings = crate::settings::Settings::default();
    let patched = update_settings(
        &settings,
        &json!({"quality":"High","encoder":{"vp9_cpu_used":2}}),
    )
    .unwrap();
    assert_eq!(
        patched.anim,
        crate::export::AnimOptions::preset(crate::export::Quality::High)
    );
    assert_eq!(patched.shot_template, settings.shot_template);
    assert!(!patched.mcp_enabled);
    assert_eq!(patched.encoder.x264_preset, settings.encoder.x264_preset);
    assert_eq!(patched.encoder.vp9_cpu_used, 2);
}
#[test]
fn headless_gui_tools_and_unopened_media_return_actionable_errors() {
    let mut engine = Engine::new();
    assert!(
        engine
            .call(&Request {
                name: "gui_get_state".into(),
                arguments: json!({})
            })
            .unwrap_err()
            .to_string()
            .contains("GUI unavailable")
    );
    assert!(
        engine
            .call(&Request {
                name: "media_info".into(),
                arguments: json!({})
            })
            .unwrap_err()
            .to_string()
            .contains("Open media")
    );
    assert!(
        engine
            .call(&Request {
                name: "media_open".into(),
                arguments: json!({"path":"/does/not/exist"})
            })
            .is_err()
    );
}
#[cfg(unix)]
#[test]
fn ipc_connect_disconnect_permissions_and_shutdown() {
    use std::{thread, time::Duration};
    let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 999_101).unwrap();
    let path = scratch.path.join("gui.sock");
    let bridge = ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).unwrap();
    let request = Request {
        name: "gui_get_state".into(),
        arguments: json!({}),
    };
    let client = thread::spawn(move || ipc::call(&path, &request));
    let command = bridge
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    command
        .reply
        .send(Ok(json!({"gui":true,"in_seconds":1.0,"out_seconds":2.0})))
        .unwrap();
    assert_eq!(client.join().unwrap().unwrap()["in_seconds"], 1.0);
    drop(bridge);
    assert!(!scratch.path.join("gui.sock").exists());
    assert!(
        ipc::call(
            &scratch.path.join("gui.sock"),
            &Request {
                name: "gui_get_state".into(),
                arguments: json!({})
            }
        )
        .is_err()
    );
    // Disabling a bridge drops queued commands without applying them or waiting
    // for a UI response, and rejects subsequent clients.
    let path = scratch.path.join("gui.sock");
    let bridge = ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).unwrap();
    let client = thread::spawn(move || {
        ipc::call(
            &path,
            &Request {
                name: "gui_get_state".into(),
                arguments: json!({}),
            },
        )
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while bridge.requests.is_empty() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!bridge.requests.is_empty());
    let shutdown_started = std::time::Instant::now();
    drop(bridge);
    assert!(shutdown_started.elapsed() < Duration::from_secs(1));
    assert!(client.join().unwrap().is_err());
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&scratch.path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        ipc::Bridge::start(
            scratch.path.join("unsafe.sock"),
            eframe::egui::Context::default()
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn ipc_recovers_abandoned_socket_and_preserves_live_legacy_listener() {
    use std::os::unix::{fs::MetadataExt, net::UnixListener};
    let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 999_111).unwrap();
    let path = scratch.path.join("gui.sock");
    drop(UnixListener::bind(&path).unwrap()); // Same residue as a killed GUI.
    let bridge = ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).unwrap();
    assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
    drop(bridge);
    assert!(!path.exists());
    // An old version may own a listener without the new advisory lock.
    let legacy = UnixListener::bind(&path).unwrap();
    let original = std::fs::symlink_metadata(&path).unwrap().ino();
    let started = std::time::Instant::now();
    assert!(ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(std::fs::symlink_metadata(&path).unwrap().ino(), original);
    assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
    drop(legacy);
}

#[cfg(unix)]
#[test]
fn ipc_serializes_concurrent_owners_and_preserves_replaced_paths_on_drop() {
    use std::{
        os::unix::{fs::MetadataExt, net::UnixListener},
        sync::{Arc, Barrier},
        thread,
    };
    let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 999_112).unwrap();
    let path = scratch.path.join("gui.sock");
    drop(UnixListener::bind(&path).unwrap());
    let barrier = Arc::new(Barrier::new(8));
    let callers = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            let path = path.clone();
            thread::spawn(move || {
                barrier.wait();
                ipc::Bridge::start(path, eframe::egui::Context::default()).ok()
            })
        })
        .collect::<Vec<_>>();
    let mut owners = callers
        .into_iter()
        .filter_map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(owners.len(), 1, "Only one GUI may own/reclaim the socket");
    let bridge = owners.pop().unwrap();
    assert!(ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).is_err());
    std::fs::remove_file(&path).unwrap();
    let replacement = UnixListener::bind(&path).unwrap();
    let replaced_inode = std::fs::symlink_metadata(&path).unwrap().ino();
    drop(bridge);
    assert_eq!(
        std::fs::symlink_metadata(&path).unwrap().ino(),
        replaced_inode
    );
    assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
    drop(replacement);
    // The OS releases the advisory lock; the persistent lock file is reusable.
    drop(ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).unwrap());
    assert!(!path.exists());
    assert!(scratch.path.join("gui.sock.lock").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn ipc_preserves_legacy_listener_with_a_full_accept_backlog() {
    use std::{
        os::{
            fd::AsRawFd,
            unix::{fs::MetadataExt, net::UnixListener},
        },
        time::{Duration, Instant},
    };
    let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 999_114).unwrap();
    let path = scratch.path.join("gui.sock");
    let _legacy = UnixListener::bind(&path).unwrap();
    // std uses the system backlog limit (potentially thousands). Reduce this
    // test listener's backlog so saturation is deterministic under low fd limits.
    unsafe extern "C" {
        fn listen(socket: std::ffi::c_int, backlog: std::ffi::c_int) -> std::ffi::c_int;
    }
    // SAFETY: this is a live, owned AF_UNIX listening socket; listen only updates
    // its backlog and does not transfer ownership or access Rust memory.
    assert_eq!(unsafe { listen(_legacy.as_raw_fd(), 1) }, 0);
    let original = std::fs::symlink_metadata(&path).unwrap().ino();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let connections = runtime.block_on(async {
        let mut connections = Vec::new();
        for _ in 0..16 {
            match tokio::time::timeout(
                Duration::from_millis(20),
                tokio::net::UnixStream::connect(&path),
            )
            .await
            {
                Ok(Ok(connection)) => connections.push(connection),
                _ => return connections,
            }
        }
        panic!("Did not saturate the legacy listener backlog");
    });
    assert!(!connections.is_empty());
    let started = Instant::now();
    assert!(ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(std::fs::symlink_metadata(&path).unwrap().ino(), original);
}

#[cfg(unix)]
#[test]
fn ipc_recovery_refuses_files_symlinks_and_unsafe_locks() {
    use std::os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    };
    let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 999_113).unwrap();
    let path = scratch.path.join("gui.sock");
    std::fs::write(&path, b"preserve this file").unwrap();
    assert!(ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"preserve this file");
    std::fs::remove_file(&path).unwrap();
    let target = scratch.path.join("target.sock");
    drop(UnixListener::bind(&target).unwrap());
    symlink(&target, &path).unwrap();
    assert!(ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).is_err());
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(target.exists());
    std::fs::remove_file(&path).unwrap();
    let lock = scratch.path.join("gui.sock.lock");
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ipc::Bridge::start(path.clone(), eframe::egui::Context::default()).is_err());
    std::fs::remove_file(&lock).unwrap();
    symlink(&target, &lock).unwrap();
    assert!(ipc::Bridge::start(path, eframe::egui::Context::default()).is_err());
    assert!(target.exists());
}

#[cfg(unix)]
#[test]
fn saturated_gui_lane_does_not_starve_cached_status_or_apply_disconnected_commands() {
    use std::{
        io::Write,
        os::unix::net::UnixStream,
        thread,
        time::{Duration, Instant},
    };
    let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 999_102).unwrap();
    let path = scratch.path.join("gui.sock");
    let service = Arc::new(service::Service::new(crate::export::Tools {
        ffmpeg: "unused".into(),
        gifsicle: None,
    }));
    let bridge = ipc::Bridge::with_service(
        path.clone(),
        eframe::egui::Context::default(),
        Some(service),
    )
    .unwrap();
    let callers = (0..ipc::COMMAND_CAPACITY)
        .map(|_| {
            let path = path.clone();
            thread::spawn(move || {
                ipc::call(
                    &path,
                    &Request {
                        name: "gui_get_state".into(),
                        arguments: json!({}),
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    let deadline = Instant::now() + Duration::from_secs(2);
    while bridge.requests.len() < ipc::COMMAND_CAPACITY && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(bridge.requests.len(), ipc::COMMAND_CAPACITY);
    let start = Instant::now();
    let status = ipc::call(
        &path,
        &Request {
            name: "jobs_list".into(),
            arguments: json!({}),
        },
    )
    .unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(status["jobs"], json!([]));
    let full = ipc::call(
        &path,
        &Request {
            name: "gui_get_state".into(),
            arguments: json!({}),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(full.contains("queue full"), "{full}");
    for request in bridge.requests.try_iter() {
        request.reply.send(Ok(json!({"gui":true}))).unwrap();
    }
    for caller in callers {
        assert!(caller.join().unwrap().is_ok());
    }
    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .write_all(b"{\"name\":\"gui_seek\",\"arguments\":{\"seconds\":1}}\n")
        .unwrap();
    let pending = bridge
        .requests
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(1);
    while pending.valid() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !pending.valid(),
        "disconnected caller must not apply a stale seek"
    );
    drop(bridge);
}

#[cfg(unix)]
#[test]
fn export_capacity_is_bounded_and_queued_and_running_jobs_cancel_cleanly() {
    use std::{
        os::unix::fs::PermissionsExt,
        thread,
        time::{Duration, Instant},
    };
    let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 999_103).unwrap();
    let program = scratch.path.join("slow-ffmpeg");
    std::fs::write(&program, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = Engine::with_tools(crate::export::Tools {
        ffmpeg: program,
        gifsicle: None,
    });
    engine.queue_capacity = 2;
    engine.media = Some(Media {
        path: scratch.path.join("input.mkv"),
        info: json!({"streams":[]}),
        duration: 3.0,
        fps: 8.0,
        width: 128,
        height: 96,
        audio: None,
        subtitle: None,
        external_subtitle: None,
    });
    let mut ids = Vec::new();
    for n in 0..2 {
        let result=engine.call(&Request{name:"screenshot_extract".into(),arguments:json!({"output":scratch.path.join(format!("{n}.png")),"subtitles":false})}).unwrap();
        ids.push(result["job_id"].as_u64().unwrap());
    }
    let error = engine
        .call(&Request {
            name: "screenshot_extract".into(),
            arguments: json!({"output":scratch.path.join("full.png"),"subtitles":false}),
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("queue full"), "{error}");
    assert!(
        engine
            .configure_tools(crate::export::Tools {
                ffmpeg: "different".into(),
                gifsicle: None
            })
            .is_err()
    );
    for id in &ids {
        engine
            .call(&Request {
                name: "jobs_cancel".into(),
                arguments: json!({"job_id":id}),
            })
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let jobs = engine
            .call(&Request {
                name: "jobs_list".into(),
                arguments: json!({}),
            })
            .unwrap();
        if jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["status"] == "cancelled")
        {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!scratch.path.join("0.png").exists());
    assert!(!scratch.path.join("1.png").exists());
    engine
        .configure_tools(crate::export::Tools {
            ffmpeg: "different".into(),
            gifsicle: None,
        })
        .unwrap();
    assert_eq!(engine.tools().ffmpeg, std::path::PathBuf::from("different"));
    assert_eq!(
        engine
            .call(&Request {
                name: "jobs_get".into(),
                arguments: json!({"job_id":ids[0]})
            })
            .unwrap()["status"],
        "cancelled"
    );
}

#[test]
fn track_namespaces_are_explicit_and_errors_enumerate_valid_choices() {
    use crate::tracks::{Track, TrackKind};
    let tracks = vec![Track {
        kind: TrackKind::Sub,
        id: 1,
        ff_index: Some(2),
        lang: Some("eng".into()),
        title: Some("English ASS".into()),
        codec: "ass".into(),
        external: None,
        channels: None,
    }];
    let error = identifiers::gui_id(&tracks, TrackKind::Sub, Some(2), "mpv")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Valid subtitle track IDs (mpv): 1") && error.contains("English ASS"));
    assert_eq!(
        identifiers::gui_id(&tracks, TrackKind::Sub, Some(2), "ffprobe").unwrap(),
        Some(1)
    );
    let metadata = identifiers::gui_tracks(&tracks, None, Some(1));
    assert_eq!(metadata[0]["mpv_id"], 1);
    assert_eq!(metadata[0]["ffprobe_index"], 2);
    assert_eq!(metadata[0]["selected"], true);
}
