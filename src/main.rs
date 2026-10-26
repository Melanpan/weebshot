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
        format!("weebshot {} starting", env!("CARGO_PKG_VERSION")),
    );

    // `weebshot --docs-screenshots <video> <out-dir>` regenerates docs/images.
    let (initial_file, script) = match args.as_slice() {
        [flag, video, out] if flag.as_os_str() == "--docs-screenshots" => {
            (Some(video.clone()), Some(app::Script::new(out.clone())))
        }
        [file, ..] => (Some(file.clone()), None),
        [] => (None, None),
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("weebshot")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true)
        .with_icon(app::app_icon());
    if script.is_some() {
        viewport = viewport.with_resizable(false);
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
