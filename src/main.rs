#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod export;
mod fonts;
mod keymap;
mod log;
mod player;
mod settings;
mod sounds;
mod subs;
mod theme;
mod thumbs;
mod tracks;
mod update;
mod util;
mod waveform;

use eframe::egui;
use std::path::PathBuf;

fn main() -> eframe::Result {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    log::info(
        "app",
        format!("Weebshot {} starting", env!("CARGO_PKG_VERSION")),
    );

    // `weebshot --docs-screenshots <video> <out-dir> [--at <time>]` regenerates docs/images,
    // built around the subtitle line at <time> (e.g. `4:29`).
    let (initial_file, script) = match args.as_slice() {
        [flag, video, out, rest @ ..] if flag.as_os_str() == "--docs-screenshots" => {
            let at = match rest {
                [at_flag, time] if at_flag.as_os_str() == "--at" => {
                    util::parse_position(&time.to_string_lossy(), 24.0)
                }
                _ => None,
            };
            (Some(video.clone()), Some(app::Script::new(out.clone(), at)))
        }
        [file, ..] => (Some(file.clone()), None),
        [] => (None, None),
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Weebshot")
        .with_inner_size([1600.0, 960.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true)
        .with_icon(app::app_icon());
    if script.is_some() {
        // Docs screenshots: a fixed, reproducible window.
        viewport = viewport
            .with_inner_size([1440.0, 900.0])
            .with_resizable(false);
    } else {
        // An editor wants the whole screen; once resized, eframe remembers the user's size.
        viewport = viewport.with_maximized(true);
    }
    let options = eframe::NativeOptions {
        viewport,
        // The screenshot run must not read or write the user's window state.
        persist_window: script.is_none(),
        ..Default::default()
    };

    eframe::run_native(
        "weebshot",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial_file, script)?))),
    )
}
