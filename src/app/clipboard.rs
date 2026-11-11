use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use eframe::egui;
use image::RgbaImage;

use super::{App, JobState};
use crate::{
    export::{self, SubMode, ffmpeg::StillJob},
    settings::ShotFormat,
};

#[derive(Default)]
pub(super) struct State {
    backend: Option<arboard::Clipboard>,
    image: Option<crossbeam_channel::Receiver<Result<RgbaImage, String>>>,
    clips: Vec<(u64, PathBuf)>,
    completed_clips: Vec<PathBuf>,
}

impl State {
    fn backend(&mut self) -> Result<&mut arboard::Clipboard, arboard::Error> {
        if self.backend.is_none() {
            self.backend = Some(arboard::Clipboard::new()?);
        }
        Ok(self.backend.as_mut().unwrap())
    }

    pub(super) fn queue_clips(&mut self, clips: Vec<(u64, PathBuf)>) {
        self.clips = clips;
        self.completed_clips.clear();
    }

    pub(super) fn clip_finished(&mut self, id: u64, success: bool) {
        let Some(index) = self.clips.iter().position(|(job, _)| *job == id) else {
            return;
        };
        if success {
            self.completed_clips.push(self.clips.remove(index).1);
        } else {
            self.clips.clear();
            self.completed_clips.clear();
        }
    }
}

struct ScratchShot(PathBuf);

impl Drop for ScratchShot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("frame.png"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

pub(super) fn read_image(path: &Path) -> Result<RgbaImage, String> {
    image::open(path)
        .map(|image| image.into_rgba8())
        .map_err(|error| error.to_string())
}

impl App {
    fn image_copy_pending(&self) -> bool {
        self.clipboard.image.is_some()
            || self.jobs.iter().any(|job| {
                job.output.is_none()
                    && matches!(job.state, JobState::Queued | JobState::Running { .. })
            })
    }

    pub(super) fn set_clipboard_image(&mut self, image: RgbaImage) -> Result<(), arboard::Error> {
        let (width, height) = image.dimensions();
        self.clipboard.backend()?.set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(image.into_raw()),
        })
    }

    pub(super) fn load_clipboard_image(
        &mut self,
        ctx: &egui::Context,
        load: impl FnOnce() -> Result<RgbaImage, String> + Send + 'static,
    ) {
        if self.image_copy_pending() {
            return self.error("Wait for the current image copy to finish");
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load());
            ctx.request_repaint();
        });
        self.clipboard.image = Some(rx);
    }

    pub(super) fn copy_screenshot(&mut self, ctx: &egui::Context) {
        if self.image_copy_pending() {
            return self.error("Wait for the current image copy to finish");
        }
        // An image clipboard holds one still. Both uses the burned-in variant.
        let burn = self.effective_sub_mode(self.settings.shot_subs) != SubMode::Off;
        if self.settings.shot_anime4k.enabled {
            return match self.queue_upscaled_frame(burn, None) {
                Ok(()) => self.info("Anime4K frame copy queued"),
                Err(error) => self.error(error),
            };
        }
        let Some(input) = self.player.file.clone() else {
            return self.error("Open a video first");
        };
        let id = self.next_job_id;
        self.next_job_id += 1;
        let dir = std::env::temp_dir().join(format!("weebshot-copy-{}-{id}", std::process::id()));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        if let Err(error) = builder.create(&dir) {
            return self.error(format!("Could not prepare screenshot: {error}"));
        }
        let scratch = ScratchShot(dir);
        let path = scratch.0.join("frame.png");
        if self.settings.shot_crop
            && let Some(crop) = self.view_crop()
        {
            if self.tool_status.ffmpeg.is_err() {
                return self.error("Cropped screenshots need ffmpeg");
            }
            let subs = if burn {
                match self.sub_source() {
                    Ok(source) => source,
                    Err(error) => return self.error(error),
                }
            } else {
                None
            };
            let job = StillJob {
                input,
                time: self.player.time,
                fps: self.player.fps,
                crop: Some(crop),
                subs,
                output: path,
            };
            let ffmpeg = self.tools.ffmpeg.clone();
            self.load_clipboard_image(ctx, move || {
                let _scratch = scratch;
                export::run_still(&ffmpeg, &job)?;
                read_image(&job.output)
            });
        } else {
            if let Err(error) = self.player.screenshot(&path, ShotFormat::Png, burn) {
                return self.error(format!("{error:#}"));
            }
            self.load_clipboard_image(ctx, move || {
                let _scratch = scratch;
                read_image(&path)
            });
        }
    }

    pub(super) fn poll_clipboard(&mut self) {
        if let Some(result) = self
            .clipboard
            .image
            .as_ref()
            .and_then(|rx| match rx.try_recv() {
                Ok(result) => Some(result),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    Some(Err("Image copy worker stopped".into()))
                }
            })
        {
            self.clipboard.image = None;
            let result = result.and_then(|image| {
                self.set_clipboard_image(image)
                    .map_err(|error| error.to_string())
            });
            match result {
                Ok(()) => self.info("Copied image to clipboard"),
                Err(error) => self.error(format!("Could not copy image: {error}")),
            }
        }
        if self.clipboard.clips.is_empty() && !self.clipboard.completed_clips.is_empty() {
            let paths = std::mem::take(&mut self.clipboard.completed_clips);
            match self
                .clipboard
                .backend()
                .and_then(|clipboard| clipboard.set().file_list(&paths))
            {
                Ok(()) => self.info("Copied exported clips to clipboard"),
                Err(error) => self.error(format!(
                    "Clips saved, but could not copy to clipboard: {error}"
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_every_requested_clip_and_ignores_unrelated_jobs() {
        let mut clipboard = State::default();
        let raw = PathBuf::from("raw.mp4");
        let subbed = PathBuf::from("subbed.mp4");
        clipboard.queue_clips(vec![(1, subbed.clone()), (2, raw.clone())]);
        clipboard.clip_finished(3, false);
        clipboard.clip_finished(1, true);
        assert_eq!(clipboard.clips.len(), 1);
        clipboard.clip_finished(2, true);
        assert!(clipboard.clips.is_empty());
        assert_eq!(clipboard.completed_clips, [subbed, raw]);
    }

    #[test]
    fn cancelled_variant_clears_pending_clipboard_copy() {
        let mut clipboard = State::default();
        clipboard.queue_clips(vec![(1, "subbed.mp4".into()), (2, "raw.mp4".into())]);
        clipboard.clip_finished(1, true);
        clipboard.clip_finished(2, false);
        assert!(clipboard.clips.is_empty());
        assert!(clipboard.completed_clips.is_empty());
    }
}
