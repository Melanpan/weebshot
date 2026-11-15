//! Audio waveform for the timeline: ffmpeg decodes the selected audio track to low-rate
//! mono in the background, and we keep the peak level of every 5 ms.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;

use crate::export::hide_console;
use crate::log;

/// Peak values per second of audio.
pub const PEAKS_PER_SEC: f64 = 200.0;
const SAMPLE_RATE: u32 = 8000;
const SAMPLES_PER_PEAK: usize = (SAMPLE_RATE as f64 / PEAKS_PER_SEC) as usize;

/// Pipe reads need not end on a sample boundary.
fn decode_samples(bytes: &[u8], pending: &mut Option<u8>, samples: &mut Vec<i16>) {
    let mut bytes = bytes;
    if let Some((&first, rest)) = bytes.split_first()
        && let Some(low) = pending.take()
    {
        samples.push(i16::from_le_bytes([low, first]));
        bytes = rest;
    }
    let (pairs, remainder) = bytes.as_chunks::<2>();
    samples.extend(pairs.iter().map(|pair| i16::from_le_bytes(*pair)));
    if let Some(&byte) = remainder.first() {
        *pending = Some(byte);
    }
}

pub struct Waveform {
    file: PathBuf,
    ff_index: i64,
    peaks: Vec<f32>,
    rx: Receiver<Vec<f32>>,
    done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Waveform {
    pub fn spawn(ffmpeg: PathBuf, file: PathBuf, ff_index: i64, ctx: egui::Context) -> Self {
        let (tx, rx) = unbounded();
        let done = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (done2, cancel2, path) = (done.clone(), cancel.clone(), file.clone());
        thread::spawn(move || {
            let mut cmd = Command::new(&ffmpeg);
            cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
                .arg(&path)
                .args(["-map", &format!("0:{ff_index}"), "-vn", "-sn", "-dn"])
                .args(["-ac", "1", "-ar", &SAMPLE_RATE.to_string()])
                .args(["-f", "s16le", "-acodec", "pcm_s16le", "pipe:1"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            hide_console(&mut cmd);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    log::warn("waveform", format!("ffmpeg: {e}"));
                    done2.store(true, Ordering::Relaxed);
                    return;
                }
            };
            let mut stdout = child.stdout.take().unwrap();
            // A few seconds of audio per read, sent to the UI as it arrives.
            let mut buf = vec![0u8; SAMPLES_PER_PEAK * 2 * 1000];
            let mut carry: Vec<i16> = Vec::new();
            let mut pending_byte = None;
            let mut total = 0usize;
            loop {
                if cancel2.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break;
                }
                let n = match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                decode_samples(&buf[..n], &mut pending_byte, &mut carry);
                let whole = carry.len() / SAMPLES_PER_PEAK * SAMPLES_PER_PEAK;
                let peaks: Vec<f32> = carry[..whole]
                    .as_chunks::<SAMPLES_PER_PEAK>()
                    .0
                    .iter()
                    .map(|c| c.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0)
                    .collect();
                carry.drain(..whole);
                total += peaks.len();
                if tx.send(peaks).is_err() {
                    let _ = child.kill();
                    break;
                }
                ctx.request_repaint();
            }
            let _ = child.wait();
            done2.store(true, Ordering::Relaxed);
            ctx.request_repaint();
            log::info(
                "waveform",
                format!("{:.0}s of audio analysed", total as f64 / PEAKS_PER_SEC),
            );
        });
        Self {
            file,
            ff_index,
            peaks: Vec::new(),
            rx,
            done,
            cancel,
        }
    }

    pub fn matches(&self, file: &Path, ff_index: i64) -> bool {
        self.file == file && self.ff_index == ff_index
    }

    pub fn poll(&mut self) {
        while let Ok(chunk) = self.rx.try_recv() {
            self.peaks.extend(chunk);
        }
    }

    /// How much of a `duration`-second track has been analysed (0…1).
    pub fn progress(&self, duration: f64) -> f32 {
        if duration <= 0.0 {
            return 0.0;
        }
        (self.peaks.len() as f64 / PEAKS_PER_SEC / duration).min(1.0) as f32
    }

    pub fn loading(&self) -> bool {
        !self.done.load(Ordering::Relaxed)
    }

    /// Loudest peak (0…1) between `a` and `b` seconds, if that part is decoded yet.
    pub fn peak(&self, a: f64, b: f64) -> Option<f32> {
        let i = (a.max(0.0) * PEAKS_PER_SEC) as usize;
        let j = ((b * PEAKS_PER_SEC).ceil() as usize).max(i + 1);
        if i >= self.peaks.len() {
            return None;
        }
        self.peaks[i..j.min(self.peaks.len())]
            .iter()
            .copied()
            .reduce(f32::max)
    }
}

impl Drop for Waveform {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample_decoding_survives_odd_and_empty_reads() {
        let expected = [i16::MIN, -12345, -1, 0, 1, 12345, i16::MAX];
        let bytes: Vec<_> = expected.iter().flat_map(|s| s.to_le_bytes()).collect();
        for size in 1..=bytes.len() {
            let mut pending = None;
            let mut samples = Vec::new();
            for chunk in bytes.chunks(size) {
                decode_samples(chunk, &mut pending, &mut samples);
                decode_samples(&[], &mut pending, &mut samples);
            }
            assert_eq!(samples, expected, "chunk size {size}");
            assert!(pending.is_none());
        }
    }
}
