//! Little UI sounds (screenshot taken, export done/failed), played through a second,
//! audio-only mpv instance so no extra audio library is needed. The shutter is a
//! recorded sample (assets/sounds); the chimes are synthesized at startup.

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

use libmpv2::Mpv;

use crate::log;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Screenshot,
    ExportDone,
    ExportFailed,
}

impl Sound {
    pub const ALL: [Self; 3] = [Self::Screenshot, Self::ExportDone, Self::ExportFailed];

    pub fn label(self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot",
            Self::ExportDone => "Export done",
            Self::ExportFailed => "Export failed",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Screenshot => "shutter.wav",
            Self::ExportDone => "done.wav",
            Self::ExportFailed => "failed.wav",
        }
    }
}

const RATE: u32 = 44_100;

/// Camera shutter click (CC0, see assets/sounds/README.md).
const SHUTTER: &[u8] = include_bytes!("../assets/sounds/shutter.wav");

pub struct Sounds {
    mpv: Option<Mpv>,
    dir: PathBuf,
}

impl Sounds {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join("weebshot-sounds");
        let written = std::fs::create_dir_all(&dir).and_then(|()| {
            for s in Sound::ALL {
                let path = dir.join(s.file_name());
                match s {
                    Sound::Screenshot => std::fs::write(&path, SHUTTER)?,
                    _ => write_wav(&path, &synth(s))?,
                }
            }
            Ok(())
        });
        let mpv = match written {
            Ok(()) => Mpv::with_initializer(|init| {
                init.set_option("config", "no")?;
                init.set_option("terminal", "no")?;
                init.set_option("video", "no")?;
                init.set_option("vo", "null")?;
                init.set_option("idle", "yes")?;
                init.set_option("audio-display", "no")?;
                init.set_option("input-default-bindings", "no")?;
                Ok(())
            })
            .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match mpv {
            Ok(mpv) => Self {
                mpv: Some(mpv),
                dir,
            },
            Err(e) => {
                log::warn("sounds", format!("sounds disabled: {e}"));
                Self { mpv: None, dir }
            }
        }
    }

    /// Play `sound` at `volume` (0–100).
    pub fn play(&self, sound: Sound, volume: f64) {
        let Some(mpv) = &self.mpv else { return };
        let path = self.dir.join(sound.file_name());
        let _ = mpv.set_property("volume", volume);
        let _ = mpv.command("loadfile", &[&path.to_string_lossy(), "replace"]);
    }
}

/// Mono samples in -1..1.
fn synth(sound: Sound) -> Vec<f32> {
    let len = |secs: f32| (secs * RATE as f32) as usize;
    let t = |i: usize| i as f32 / RATE as f32;
    // A soft bell-ish note: sine + a little octave, quick attack, exponential decay.
    let note = |out: &mut Vec<f32>, start: f32, freq: f32, dur: f32, gain: f32| {
        let offset = len(start);
        let n = len(dur);
        if out.len() < offset + n {
            out.resize(offset + n, 0.0);
        }
        for i in 0..n {
            let x = t(i);
            let env = (x / 0.005).min(1.0) * (-x * 7.0 / dur).exp();
            let v = (TAU * freq * x).sin() + 0.25 * (TAU * 2.0 * freq * x).sin();
            out[offset + i] += v * env * gain;
        }
    };

    let mut out = Vec::new();
    match sound {
        Sound::Screenshot => {} // recorded sample, see SHUTTER
        Sound::ExportDone => {
            note(&mut out, 0.0, 659.25, 0.35, 0.3); // E5
            note(&mut out, 0.11, 987.77, 0.55, 0.3); // B5
        }
        Sound::ExportFailed => {
            note(&mut out, 0.0, 392.0, 0.3, 0.32); // G4
            note(&mut out, 0.16, 293.66, 0.45, 0.32); // D4
        }
    }
    out
}

fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    std::fs::write(path, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutter_sample_is_a_wav() {
        assert_eq!(&SHUTTER[..4], b"RIFF");
        assert_eq!(&SHUTTER[8..12], b"WAVE");
    }

    #[test]
    fn sounds_are_short_and_in_range() {
        for s in [Sound::ExportDone, Sound::ExportFailed] {
            let samples = synth(s);
            assert!(!samples.is_empty());
            assert!(samples.len() < RATE as usize, "{s:?} longer than a second");
            assert!(samples.iter().all(|v| v.abs() <= 1.0), "{s:?} clips");
        }
    }
}
