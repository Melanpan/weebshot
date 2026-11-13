//! Bounded FFmpeg frame decoding for Stitch.
use super::check_cancel;
use crate::decode::pipe;
use anyhow::{Context, Result, bail};
use image::{GrayImage, RgbImage};
use std::{io::Read, path::Path, sync::atomic::AtomicBool};

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
