//! Settings and process arguments for the bundled Anime4KCPP helper.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, bail};

use serde::{Deserialize, Serialize};

use super::{Scratch, Step, SubSource, Tool, Tools, ffmpeg::StillJob, run_steps, save_image_data};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub enabled: bool,
    pub model: Model,
    pub speed: Speed,
    pub variant: Variant,
    pub device: Device,
}

macro_rules! choices {
    ($name:ident, $default:ident, $($value:ident => $label:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $($value),+ }

        impl Default for $name {
            fn default() -> Self { Self::$default }
        }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$value),+];

            pub fn label(self) -> &'static str {
                match self { $(Self::$value => $label),+ }
            }
        }
    };
}

choices!(Model, AcNet, AcNet => "ACNet", ArNet => "ARNet");
choices!(Speed, Balanced, Fast => "Fast", Balanced => "Balanced", Slow => "Slow");
choices!(
    Variant, Neutral,
    Neutral => "Neutral",
    Denoise => "Denoise",
    Lines => "Line restoration",
    LinesDenoise => "Line restoration + denoise",
);
choices!(Device, Auto, Auto => "Auto", Gpu => "GPU", Cpu => "CPU");

impl Options {
    fn model_name(self) -> String {
        let blocks = match (self.model, self.speed) {
            (Model::AcNet, Speed::Fast) => 4,
            (Model::AcNet, Speed::Balanced) | (Model::ArNet, Speed::Fast) => 8,
            (Model::AcNet, Speed::Slow) => 18,
            (Model::ArNet, Speed::Balanced) => 16,
            (Model::ArNet, Speed::Slow) => 32,
        };
        let suffix = match self.variant {
            Variant::Neutral => "",
            Variant::Denoise => "-hdn",
            Variant::Lines => "-box",
            Variant::LinesDenoise => "-box-hdn",
        };
        format!(
            "{}-f8b{blocks}{suffix}",
            self.model.label().to_ascii_lowercase()
        )
    }

    pub(super) fn step(self, mode: &str, input: &Path, output: &Path) -> Step {
        Step {
            tool: Tool::Anime4k,
            args: vec![
                mode.into(),
                self.device.label().to_ascii_lowercase(),
                self.model_name(),
                input.to_string_lossy().into_owned(),
                output.to_string_lossy().into_owned(),
            ],
            label: "Upscaling (Anime4K)",
            weight: 3.0,
            reports_progress: mode == "video",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Frame {
    /// Captured synchronously from mpv to snapshot the frame and subtitles on screen.
    Captured(Arc<[u8]>),
    Extract(StillJob),
}

#[derive(Clone, Debug)]
pub struct Job {
    pub input: PathBuf,
    pub frame: Frame,
    pub options: Options,
    pub output: Option<PathBuf>,
}

impl Job {
    pub(super) fn run(
        &self,
        tools: &Tools,
        id: u64,
        cancel: &AtomicBool,
        report: impl Fn(f32, Option<f32>, &'static str),
    ) -> Result<Option<image::RgbaImage>> {
        let scratch = Scratch::new(&std::env::temp_dir(), id)?;
        let input = scratch.path.join("frame.png");
        let upscaled = scratch.path.join("upscaled.png");
        let mut steps = Vec::new();
        match &self.frame {
            Frame::Captured(data) => std::fs::write(&input, data)?,
            Frame::Extract(job) => {
                let mut job = job.clone();
                job.output = input.clone();
                SubSource::prepare(&mut job.subs, &scratch)?;
                steps.push(Step {
                    tool: Tool::Ffmpeg,
                    args: job.args(),
                    label: "Preparing screenshot",
                    weight: 1.0,
                    reports_progress: false,
                });
            }
        }
        steps.push(self.options.step("image", &input, &upscaled));
        let mut encoded = upscaled.clone();
        if let Some(destination) = &self.output
            && destination.extension().is_some_and(|ext| ext != "png")
        {
            encoded = scratch
                .path
                .join("encoded")
                .with_extension(destination.extension().unwrap());
            let job = StillJob {
                input: upscaled,
                output: encoded.clone(),
                time: 0.0,
                fps: 1.0,
                crop: None,
                subs: None,
            };
            steps.push(Step {
                tool: Tool::Ffmpeg,
                args: job.args(),
                label: "Encoding screenshot",
                weight: 1.0,
                reports_progress: false,
            });
        }
        run_steps(tools, 1.0, &steps, cancel, &report)?;
        if let Some(destination) = &self.output {
            if destination.exists() {
                bail!("Output already exists: {}", destination.display());
            }
            save_image_data(
                &std::fs::read(encoded)?,
                destination,
                &self.input,
                id,
                cancel,
            )?;
            Ok(None)
        } else {
            let image = image::open(encoded)?.into_rgba8();
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            Ok(Some(image))
        }
    }
}

#[cfg(test)]
mod tests;
