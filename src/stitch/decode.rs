//! Bounded FFmpeg pipe decoding, with cancellation even when the decoder is silent.
use super::check_cancel;
use crate::export::hide_console;
use anyhow::{Context, Result, bail};
use crossbeam_channel::bounded;
use image::{GrayImage, RgbImage};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Read stdout on a helper thread while the owning worker polls cancellation.
/// Both pipe readers are joined and the process is reaped on every exit path.
fn pipe(
    ffmpeg: &Path,
    args: &[String],
    cancel: &AtomicBool,
    mut consume: impl FnMut(&mut dyn Read) -> Result<()>,
) -> Result<()> {
    check_cancel(cancel)?;
    let mut cmd = Command::new(ffmpeg);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn().context("Could not start FFmpeg")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = bounded(2);
    let reader = thread::spawn(move || {
        let mut stdout = stdout;
        loop {
            let mut chunk = vec![0; 64 * 1024];
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    chunk.truncate(n);
                    if tx.send(Ok(chunk)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
    });
    let errors = thread::spawn(move || {
        // Retain a bounded tail, while draining the entire pipe.
        let mut input = stderr;
        let mut tail = Vec::new();
        let mut buf = [0; 4096];
        while let Ok(n) = input.read(&mut buf) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buf[..n]);
            if tail.len() > 8192 {
                tail.drain(..tail.len() - 8192);
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    });
    struct Stream<'a> {
        rx: crossbeam_channel::Receiver<std::io::Result<Vec<u8>>>,
        chunk: std::io::Cursor<Vec<u8>>,
        cancel: &'a AtomicBool,
        /// Last time the decoder produced output. A long but steadily progressing
        /// decode (a 60 s HEVC range on a slow CPU) must not be cut off; a silent one must.
        last_output: Instant,
    }
    impl Read for Stream<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    return Err(std::io::Error::other("Cancelled"));
                }
                if self.last_output.elapsed() > Duration::from_secs(120) {
                    return Err(std::io::Error::other("FFmpeg decode timed out"));
                }
                let n = self.chunk.read(out)?;
                if n > 0 {
                    return Ok(n);
                }
                match self.rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(chunk) => {
                        self.chunk = std::io::Cursor::new(chunk?);
                        self.last_output = Instant::now();
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(0),
                    Err(_) => (),
                }
            }
        }
    }
    let result = {
        let mut stream = Stream {
            rx,
            chunk: std::io::Cursor::new(Vec::new()),
            cancel,
            last_output: Instant::now(),
        };
        consume(&mut stream)
    };
    // On errors, kill immediately. On clean EOF, allow the decoder to exit;
    // never turn a decoder failure without stderr into a successful stitch.
    if result.is_err() {
        let _ = child.kill();
    }
    let wait_started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(e);
            }
            Ok(None) => {
                if cancel.load(Ordering::Relaxed) || wait_started.elapsed() > Duration::from_secs(2)
                {
                    let _ = child.kill();
                    break child.wait();
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let _ = reader.join();
    let errors = errors.join().unwrap_or_default();
    check_cancel(cancel)?;
    result?;
    let status = status?;
    if !status.success() {
        bail!(
            "FFmpeg could not decode the video ({status}): {}",
            errors.trim()
        );
    }
    Ok(())
}

fn base(input: &Path, time: f64) -> Vec<String> {
    [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-filter_threads",
        "1",
        "-threads",
        "2",
        "-ss",
        &format!("{time:.6}"),
        "-i",
        &input.to_string_lossy(),
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-dn",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub fn frame(
    ffmpeg: &Path,
    input: &Path,
    time: f64,
    cancel: &AtomicBool,
) -> Result<(RgbImage, (u32, u32))> {
    image_frame(ffmpeg, base(input, time), cancel)
}

/// Decode selected native frames in one sequential pass, with exactly the same
/// FPS filter used for analysis (including its choice of source timestamps).
#[allow(clippy::too_many_arguments)]
pub fn selected_frames(
    ffmpeg: &Path,
    input: &Path,
    start: f64,
    end: f64,
    fps: f64,
    indices: &[usize],
    size: (u32, u32),
    cancel: &AtomicBool,
    mut receive: impl FnMut(usize, RgbImage) -> Result<()>,
) -> Result<()> {
    if indices.is_empty() {
        return Ok(());
    }
    let args = selection_args(input, start, end, fps, indices);
    pipe(ffmpeg, &args, cancel, |r| {
        for n in 0..indices.len() {
            check_cancel(cancel)?;
            let mut bytes = vec![0; (size.0 * size.1 * 3) as usize];
            r.read_exact(&mut bytes)
                .context("Video ended or changed dimensions during native frame decoding")?;
            receive(n, RgbImage::from_raw(size.0, size.1, bytes).unwrap())?;
        }
        let mut tail = [0];
        if r.read(&mut tail)? != 0 {
            bail!("Unexpected frame size in native video stream");
        }
        Ok(())
    })
}

/// Build a bounded native decode. An output `-t` alone is insufficient: after
/// select stops emitting frames, its output clock stops advancing and FFmpeg can
/// decode the entire remaining episode. Always cap the selected output count.
pub(super) fn selection_args(
    input: &Path,
    start: f64,
    end: f64,
    fps: f64,
    indices: &[usize],
) -> Vec<String> {
    // Keep one second of preroll for FPS resampling. Restore the original
    // timestamp origin before the FPS filter so seeking does not change which
    // source pictures its original sample indices refer to.
    let skip = (indices[0] as f64 / fps - 1.0).floor().max(0.0);
    let mut args = base(input, start + skip);
    let timing = if skip > 0.0 {
        format!("settb=AVTB,setpts=PTS+{skip}*1000000,")
    } else {
        String::new()
    };
    let select = indices
        .iter()
        .map(|i| format!("eq(n\\,{i})"))
        .collect::<Vec<_>>()
        .join("+");
    args.extend([
        "-t".into(),
        format!("{:.6}", end - start),
        "-vf".into(),
        format!("{timing}fps={fps}:start_time=0,select={select}"),
        "-frames:v".into(),
        indices.len().to_string(),
        "-fps_mode".into(),
        "passthrough".into(),
    ]);
    args.extend(
        [
            "-threads", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1",
        ]
        .map(str::to_owned),
    );
    args
}

fn image_frame(
    ffmpeg: &Path,
    mut args: Vec<String>,
    cancel: &AtomicBool,
) -> Result<(RgbImage, (u32, u32))> {
    args.extend(
        [
            "-threads",
            "1",
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "pipe:1",
        ]
        .map(str::to_owned),
    );
    let mut bytes = Vec::new();
    pipe(ffmpeg, &args, cancel, |r| {
        r.take(80 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 80 * 1024 * 1024 {
            bail!("Decoded frame exceeds the memory limit");
        }
        Ok(())
    })?;
    let aspect = super::png::ffmpeg_aspect(&bytes);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .context("FFmpeg returned no usable image")?
        .to_rgb8();
    if image.width() as u64 * image.height() as u64 > 16_777_216 {
        bail!("Source frame exceeds the 16 megapixel limit");
    }
    Ok((image, aspect))
}

#[allow(clippy::too_many_arguments)]
pub fn samples(
    ffmpeg: &Path,
    input: &Path,
    start: f64,
    end: f64,
    fps: f64,
    size: (u32, u32),
    cancel: &AtomicBool,
    mut receive: impl FnMut(GrayImage) -> Result<()>,
) -> Result<()> {
    let mut args = base(input, start);
    args.extend([
        "-t".into(),
        format!("{:.6}", end - start),
        "-vf".into(),
        format!(
            "fps={fps}:start_time=0,scale={}:{}:flags=area",
            size.0, size.1
        ),
    ]);
    args.extend(
        [
            "-threads", "1", "-f", "rawvideo", "-pix_fmt", "gray", "pipe:1",
        ]
        .map(str::to_owned),
    );
    pipe(ffmpeg, &args, cancel, |r| {
        let mut bytes = vec![0; (size.0 * size.1) as usize];
        loop {
            check_cancel(cancel)?;
            // Read one byte separately to distinguish clean EOF from a truncated frame.
            if r.read(&mut bytes[..1])? == 0 {
                break;
            }
            r.read_exact(&mut bytes[1..])?;
            receive(GrayImage::from_raw(size.0, size.1, bytes.clone()).unwrap())?;
        }
        Ok(())
    })
}
