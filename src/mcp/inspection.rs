//! Read-only, incremental visual inspection. No player, egui, export jobs or files.
use super::Request;
use anyhow::{Context, Result, bail, ensure};
use crossbeam_channel::{Receiver, bounded};
use image::{
    Rgb, RgbImage,
    imageops::{self, FilterType},
};
use serde_json::{Value, json};
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const CONTACT_LIMIT: usize = 64;
pub const BATCH_LIMIT: usize = 16;
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_width: u32,
    pub contact_frames: usize,
    pub batch_frames: usize,
    pub max_pixels: u64,
    pub max_response_bytes: usize,
    pub timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_width: 1920,
            contact_frames: CONTACT_LIMIT,
            batch_frames: BATCH_LIMIT,
            max_pixels: 8_000_000,
            max_response_bytes: 8 * 1024 * 1024,
            timeout: Duration::from_secs(120),
        }
    }
}
impl Limits {
    pub fn from_env() -> Result<Self> {
        let mut limits = Self::default();
        fn setting(name: &str, default: u64, min: u64, max: u64) -> Result<u64> {
            let Some(value) = std::env::var_os(name) else {
                return Ok(default);
            };
            let value = value
                .to_str()
                .and_then(|v| v.parse::<u64>().ok())
                .with_context(|| format!("{name} must be an integer"))?;
            ensure!(
                (min..=max).contains(&value),
                "{name} must be between {min} and {max}"
            );
            Ok(value)
        }
        limits.contact_frames = setting(
            "WEEBSHOT_INSPECTION_MAX_CONTACT_FRAMES",
            limits.contact_frames as u64,
            1,
            CONTACT_LIMIT as u64,
        )? as usize;
        limits.batch_frames = setting(
            "WEEBSHOT_INSPECTION_MAX_BATCH_FRAMES",
            limits.batch_frames as u64,
            1,
            BATCH_LIMIT as u64,
        )? as usize;
        limits.max_width = setting(
            "WEEBSHOT_INSPECTION_MAX_WIDTH",
            limits.max_width.into(),
            128,
            4096,
        )? as u32;
        limits.max_pixels = setting(
            "WEEBSHOT_INSPECTION_MAX_PIXELS",
            limits.max_pixels,
            16384,
            32_000_000,
        )?;
        limits.max_response_bytes = setting(
            "WEEBSHOT_INSPECTION_MAX_RESPONSE_BYTES",
            limits.max_response_bytes as u64,
            1024,
            12 * 1024 * 1024,
        )? as usize;
        limits.timeout = Duration::from_secs(setting(
            "WEEBSHOT_INSPECTION_TIMEOUT_SECONDS",
            limits.timeout.as_secs(),
            1,
            3600,
        )?);
        Ok(limits)
    }
}
#[derive(Debug)]
pub struct Video {
    pub path: PathBuf,
    pub duration: f64,
    pub display_aspect: f64,
}
impl Video {
    pub fn from_info(path: PathBuf, info: &Value) -> Result<Self> {
        let v = info["streams"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
            .context("Media contains no video")?;
        let width = v["width"].as_u64().context("Missing video width")?;
        let height = v["height"].as_u64().context("Missing video height")?;
        ensure!(
            width > 0 && height > 0 && width.saturating_mul(height) <= 32_000_000,
            "Source exceeds the 32 megapixel inspection limit"
        );
        let sar = v["sample_aspect_ratio"]
            .as_str()
            .and_then(|s| s.split_once(':'))
            .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0);
        let rotation = v["side_data_list"]
            .as_array()
            .and_then(|a| a.iter().find_map(|v| v["rotation"].as_f64()))
            .or_else(|| {
                v["tags"]["rotate"]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
            })
            .unwrap_or(0.0);
        let mut aspect = width as f64 * sar / height as f64;
        if (rotation.rem_euclid(180.0) - 90.0).abs() < 0.01 {
            aspect = 1.0 / aspect;
        }
        ensure!(
            aspect.is_finite() && aspect > 0.0,
            "Invalid display aspect ratio"
        );
        let duration = info["format"]["duration"]
            .as_str()
            .or_else(|| v["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .context("Missing media duration")?;
        ensure!(
            duration.is_finite() && duration > 0.0,
            "Invalid media duration"
        );
        Ok(Self {
            path,
            duration,
            display_aspect: aspect,
        })
    }
}
pub struct Visual {
    pub data: Vec<u8>,
    pub mime: &'static str,
    pub description: String,
}
pub struct Output {
    pub metadata: Value,
    pub images: Vec<Visual>,
}
#[derive(Clone, Copy, Debug)]
struct Stamp {
    seconds: f64,
    pts: i64,
    num: i64,
    den: i64,
}
impl Stamp {
    fn metadata(self) -> Value {
        json!({"actual_timestamp_seconds":self.seconds,"presentation_timestamp":self.pts,"time_base":{"numerator":self.num,"denominator":self.den}})
    }
}

pub fn is_tool(name: &str) -> bool {
    matches!(
        name,
        "video_contact_sheet" | "video_extract_frames" | "video_timeline_scenes"
    )
}
fn check(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Inspection cancelled");
    ensure!(
        Instant::now() < deadline,
        "Inspection timed out; request a shorter interval or fewer frames"
    );
    Ok(())
}
pub fn timecode(seconds: f64) -> String {
    let millis = (seconds * 1000.0).round().max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        millis / 3_600_000,
        millis / 60_000 % 60,
        millis / 1000 % 60,
        millis % 1000
    )
}
pub fn uniform(start: f64, end: f64, count: usize) -> Vec<f64> {
    (0..count)
        .map(|i| start + (end - start) * (i as f64 + 0.5) / count as f64)
        .collect()
}
pub fn interval(start: f64, end: f64, step: f64, max: usize) -> (Vec<f64>, bool, u64) {
    let count = ((end - start) / step).ceil().max(1.0) as u64;
    let times = (0..count.min(max as u64))
        .map(|i| start + i as f64 * step)
        .take_while(|t| *t < end)
        .collect::<Vec<_>>();
    let truncated = count > times.len() as u64;
    (times, truncated, count)
}
pub fn scene_samples(start: f64, end: f64, cuts: &[f64], max: usize) -> (Vec<f64>, bool, bool) {
    let mut boundaries = vec![start];
    for &cut in cuts {
        if cut > start && cut < end && cut > *boundaries.last().unwrap() + 1e-6 {
            boundaries.push(cut);
        }
    }
    if boundaries.len() == 1 {
        return (uniform(start, end, max), false, true);
    }
    boundaries.push(end);
    let representatives = boundaries
        .windows(2)
        .map(|s| (s[0] + s[1]) * 0.5)
        .collect::<Vec<_>>();
    let truncated = representatives.len() > max;
    let indices = scene_indices(start, end, &representatives, max);
    (
        indices.into_iter().map(|i| representatives[i]).collect(),
        truncated,
        false,
    )
}
fn scene_indices(start: f64, end: f64, representatives: &[f64], max: usize) -> Vec<usize> {
    if representatives.len() <= max {
        return (0..representatives.len()).collect();
    }
    let mut available = vec![true; representatives.len()];
    let mut chosen = Vec::new();
    for target in uniform(start, end, max) {
        let index = representatives
            .iter()
            .enumerate()
            .filter(|(i, _)| available[*i])
            .min_by(|(_, a), (_, b)| (**a - target).abs().total_cmp(&(**b - target).abs()))
            .unwrap()
            .0;
        available[index] = false;
        chosen.push(index);
    }
    chosen.sort_unstable();
    chosen
}

fn base(video: &Video, start: f64, end: f64) -> Vec<String> {
    [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "info",
        "-filter_threads",
        "1",
        "-threads",
        "2",
        "-copyts",
        "-start_at_zero",
        "-ss",
        &format!("{start:.9}"),
        "-t",
        &format!("{:.9}", (end - start).max(0.001)),
        "-protocol_whitelist",
        "file,pipe",
        "-i",
        &video.path.to_string_lossy(),
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-dn",
    ]
    .map(str::to_owned)
    .to_vec()
}
fn stamp_parser(sender: crossbeam_channel::Sender<Stamp>) -> impl FnMut(&str) + Send + 'static {
    let mut time_base = None;
    move |line| {
        if !line.contains("showinfo") {
            return;
        }
        if let Some(value) = line
            .split("config in time_base:")
            .nth(1)
            .and_then(|s| s.split(',').next())
        {
            time_base = value
                .trim()
                .split_once('/')
                .and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?)))
                .filter(|(a, b)| *a > 0 && *b > 0);
        }
        if let (Some((num, den)), Some(pts)) = (
            time_base,
            line.split(" pts:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<i64>().ok()),
        ) {
            let _ = sender.try_send(Stamp {
                seconds: pts as f64 * num as f64 / den as f64,
                pts,
                num,
                den,
            });
        }
    }
}
fn next_stamp(receiver: &Receiver<Stamp>, cancel: &AtomicBool, deadline: Instant) -> Result<Stamp> {
    loop {
        check(cancel, deadline)?;
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(s) => return Ok(s),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => bail!("Decoder omitted presentation timestamps"),
        }
    }
}
fn ppm(reader: &mut dyn Read, limits: &Limits) -> Result<Option<RgbImage>> {
    let mut first = [0];
    if reader.read(&mut first)? == 0 {
        return Ok(None);
    }
    ensure!(first[0] == b'P', "Invalid frame stream");
    fn token(reader: &mut dyn Read) -> Result<String> {
        let mut value = Vec::new();
        let mut b = [0];
        loop {
            reader.read_exact(&mut b)?;
            if b[0].is_ascii_whitespace() {
                if !value.is_empty() {
                    break;
                }
            } else {
                value.push(b[0]);
                ensure!(value.len() < 32, "Invalid frame header");
            }
        }
        Ok(String::from_utf8(value)?)
    }
    ensure!(token(reader)? == "6", "Invalid PPM format");
    let width = token(reader)?.parse::<u32>()?;
    let height = token(reader)?.parse::<u32>()?;
    ensure!(token(reader)? == "255", "Unsupported frame depth");
    let pixels = u64::from(width) * u64::from(height);
    ensure!(
        width > 0
            && height > 0
            && width <= limits.max_width
            && height <= 2160
            && pixels <= limits.max_pixels,
        "Decoded image exceeds inspection limits; request lower resolution"
    );
    let mut data = vec![0; pixels as usize * 3];
    reader
        .read_exact(&mut data)
        .context("Incomplete decoded frame")?;
    Ok(Some(RgbImage::from_raw(width, height, data).unwrap()))
}
fn scale(width: u32, upscale: bool) -> String {
    let factor = format!("min({width}/(iw*sar),2160/ih)");
    let factor = if upscale {
        factor
    } else {
        format!("min(1,{factor})")
    };
    format!(
        "scale=w='max(1,trunc(iw*sar*{factor}))':h='max(1,trunc(ih*{factor}))':flags=area,setsar=1"
    )
}
#[derive(Clone, Copy)]
struct SceneAnalysis {
    start: f64,
    end: f64,
    threshold: f64,
    min_seconds: f64,
}
fn detect_scenes(
    ffmpeg: &Path,
    video: &Video,
    analysis: SceneAnalysis,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<f64>> {
    let SceneAnalysis {
        start,
        end,
        threshold,
        min_seconds,
    } = analysis;
    let (tx, rx) = bounded(20_001);
    let mut args = base(video, start, end);
    args.extend([
        "-vf".into(),
        format!(
            "{},select='gt(scene,{threshold:.6})',showinfo=checksum=0",
            scale(320, false)
        ),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    crate::decode::pipe_with_lines(
        ffmpeg,
        &args,
        cancel,
        deadline.saturating_duration_since(Instant::now()),
        Some(deadline),
        |r| {
            let mut byte = [0];
            while r.read(&mut byte)? != 0 {
                check(cancel, deadline)?;
            }
            Ok(())
        },
        stamp_parser(tx),
    )?;
    let cuts = rx.try_iter().map(|s| s.seconds).collect::<Vec<_>>();
    ensure!(
        cuts.len() <= 20_000,
        "Scene boundary limit reached; request a shorter interval"
    );
    let mut filtered = Vec::new();
    for cut in cuts {
        if cut - start >= min_seconds
            && end - cut >= min_seconds
            && filtered.last().is_none_or(|last| cut - last >= min_seconds)
        {
            filtered.push(cut);
        }
    }
    Ok(filtered)
}
struct Decode<'a> {
    ffmpeg: &'a Path,
    video: &'a Video,
    cancel: &'a AtomicBool,
    limits: &'a Limits,
    deadline: Instant,
}
// Reuse one decoder for nearby targets. For sparse episode overviews, seek past
// large gaps rather than decoding minutes of footage between two thumbnails.
fn seek_groups(times: &[f64]) -> Vec<Vec<usize>> {
    let mut indices = (0..times.len()).collect::<Vec<_>>();
    indices.sort_by(|a, b| times[*a].total_cmp(&times[*b]));
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for index in indices {
        if groups
            .last()
            .is_none_or(|g| times[index] - times[*g.last().unwrap()] > 5.0)
        {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(index);
    }
    groups
}
fn extract(
    context: Decode<'_>,
    times: &[f64],
    width: u32,
    upscale: bool,
    mut receive: impl FnMut(usize, Stamp, &RgbImage) -> Result<()>,
) -> Result<()> {
    let mut first_error = None;
    for indices in seek_groups(times) {
        check(context.cancel, context.deadline)?;
        let group = indices.iter().map(|i| times[*i]).collect::<Vec<_>>();
        let mut callback_failed = false;
        let result = extract_span(
            Decode { ..context },
            &group,
            width,
            upscale,
            |index, stamp, image| {
                let result = receive(indices[index], stamp, image);
                callback_failed |= result.is_err();
                result
            },
        );
        if callback_failed {
            return result;
        }
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    check(context.cancel, context.deadline)?;
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}
fn extract_span(
    context: Decode<'_>,
    times: &[f64],
    width: u32,
    upscale: bool,
    mut receive: impl FnMut(usize, Stamp, &RgbImage) -> Result<()>,
) -> Result<()> {
    let Decode {
        ffmpeg,
        video,
        cancel,
        limits,
        deadline,
    } = context;
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.dedup();
    let first = sorted[0];
    let last = *sorted.last().unwrap();
    let expression = sorted
        .iter()
        .map(|t| format!("gte(t,{t:.12})*if(isnan(prev_selected_t),1,lt(prev_selected_t,{t:.12}))"))
        .collect::<Vec<_>>()
        .join("+");
    let mut args = base(video, first, (last + 1.0).min(video.duration));
    args.extend([
        "-vf".into(),
        format!(
            "select='{expression}',{},showinfo=checksum=0",
            scale(width, upscale)
        ),
        "-frames:v".into(),
        sorted.len().to_string(),
        "-fps_mode".into(),
        "passthrough".into(),
        "-threads".into(),
        "1".into(),
        "-c:v".into(),
        "ppm".into(),
        "-f".into(),
        "image2pipe".into(),
        "pipe:1".into(),
    ]);
    let (tx, rx) = bounded(CONTACT_LIMIT + 1);
    let mut matched = vec![false; times.len()];
    crate::decode::pipe_with_lines(
        ffmpeg,
        &args,
        cancel,
        deadline.saturating_duration_since(Instant::now()),
        Some(deadline),
        |reader| {
            while let Some(image) = ppm(reader, limits)? {
                let stamp = next_stamp(&rx, cancel, deadline)?;
                for (index, &target) in times.iter().enumerate() {
                    if !matched[index] && target <= stamp.seconds + 1e-9 {
                        receive(index, stamp, &image)?;
                        matched[index] = true;
                    }
                }
                check(cancel, deadline)?;
            }
            Ok(())
        },
        stamp_parser(tx),
    )
}
fn encode(image: &RgbImage, format: &str, quality: u8) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    if format == "jpeg" {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(image)?;
    } else {
        image::DynamicImage::ImageRgb8(image.clone())
            .write_to(&mut out, image::ImageFormat::Png)?;
    }
    Ok(out.into_inner())
}
fn validate_times(times: &[f64], duration: f64) -> Result<()> {
    ensure!(!times.is_empty(), "Provide at least one timestamp");
    for (i, t) in times.iter().enumerate() {
        ensure!(
            t.is_finite() && *t >= 0.0 && *t < duration,
            "timestamps_seconds[{i}] must be >= 0 and < media duration ({duration})"
        );
    }
    Ok(())
}
fn frame_metadata(index: usize, requested: f64, stamp: Stamp) -> Value {
    let mut value = stamp.metadata();
    value["index"] = json!(index);
    value["timestamp_seconds"] = json!(requested);
    value["timecode"] = json!(timecode(stamp.seconds));
    value
}
fn payload_size(bytes: usize, limits: &Limits) -> Result<()> {
    // Include base64 expansion and conservative metadata overhead in the budget.
    ensure!(
        bytes.saturating_add(2) / 3 * 4 + 65536 <= limits.max_response_bytes,
        "MCP image payload exceeds the response limit; request fewer frames, lower resolution, JPEG, or lower quality"
    );
    Ok(())
}

pub fn run(
    ffmpeg: &Path,
    video: &Video,
    request: &Request,
    cancel: &AtomicBool,
    limits: &Limits,
) -> Result<Output> {
    super::tools::validate(request)?;
    ensure!(is_tool(&request.name), "Unknown inspection tool");
    let deadline = Instant::now() + limits.timeout;
    check(cancel, deadline)?;
    if request.name == "video_timeline_scenes" {
        timeline_scenes(ffmpeg, video, &request.arguments, cancel, limits, deadline)
    } else if request.name == "video_extract_frames" {
        batch(ffmpeg, video, &request.arguments, cancel, limits, deadline)
    } else {
        sheet(ffmpeg, video, &request.arguments, cancel, limits, deadline)
    }
}
fn batch(
    ffmpeg: &Path,
    video: &Video,
    a: &Value,
    cancel: &AtomicBool,
    limits: &Limits,
    deadline: Instant,
) -> Result<Output> {
    let times = a["timestamps_seconds"]
        .as_array()
        .context("timestamps_seconds must be an array")?
        .iter()
        .map(|v| v.as_f64().context("Timestamps must be numbers"))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        times.len() <= limits.batch_frames,
        "At most {} frames per extraction under the configured limit",
        limits.batch_frames
    );
    validate_times(&times, video.duration)?;
    let width = a["max_width"].as_u64().unwrap_or(1280) as u32;
    ensure!(
        width <= limits.max_width,
        "max_width exceeds configured inspection limit ({})",
        limits.max_width
    );
    let format = a["format"].as_str().unwrap_or("jpeg");
    let quality = a["quality"].as_u64().unwrap_or(85) as u8;
    let mut encoded: Vec<Option<(Value, Visual)>> = (0..times.len()).map(|_| None).collect();
    let mut bytes = 0usize;
    let result = extract(
        Decode {
            ffmpeg,
            video,
            cancel,
            limits,
            deadline,
        },
        &times,
        width,
        false,
        |index, stamp, image| {
            let data = encode(image, format, quality)?;
            bytes = bytes.saturating_add(data.len());
            payload_size(bytes, limits)?;
            let mut meta = frame_metadata(index, times[index], stamp);
            meta["width"] = json!(image.width());
            meta["height"] = json!(image.height());
            encoded[index] = Some((
                meta,
                Visual {
                    data,
                    mime: if format == "jpeg" {
                        "image/jpeg"
                    } else {
                        "image/png"
                    },
                    description: format!(
                        "Frame {index}: requested {}, actual {}",
                        timecode(times[index]),
                        timecode(stamp.seconds)
                    ),
                },
            ));
            Ok(())
        },
    );
    check(cancel, deadline)?;
    payload_size(bytes, limits)?;
    let mut frames = Vec::new();
    let mut images = Vec::new();
    for (index, entry) in encoded.into_iter().enumerate() {
        if let Some((mut meta, image)) = entry {
            meta["image_index"] = json!(images.len());
            meta["content_index"] = json!(images.len() * 2 + 2);
            images.push(image);
            frames.push(meta);
        } else {
            frames.push(json!({"index":index,"timestamp_seconds":times[index],"error":result.as_ref().err().map(|e|format!("{e:#}")).unwrap_or_else(||"No decodable frame at or after this timestamp".into())}));
        }
    }
    ensure!(
        !images.is_empty(),
        "No frames could be decoded: {}",
        result
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_else(|| "requested timestamps contain no decodable frames".into())
    );
    Ok(Output {
        metadata: json!({"duration_seconds":video.duration,"frame_count":images.len(),"requested_count":times.len(),"format":format,"frames":frames,"partial":images.len()!=times.len(),"decoder_error":result.err().map(|e|format!("{e:#}"))}),
        images,
    })
}
struct Grid {
    image: RgbImage,
    columns: u32,
    cell_width: u32,
    thumb_height: u32,
    label_height: u32,
}
impl Grid {
    fn new(
        count: usize,
        columns: u32,
        width: u32,
        aspect: f64,
        labels: bool,
        limits: &Limits,
    ) -> Result<Self> {
        let height = (width as f64 / aspect).round().clamp(1.0, 2160.0) as u32;
        let label_height = if labels { 28 } else { 0 };
        let rows = (count as u32).div_ceil(columns);
        let total_width = columns * width;
        let total_height = rows * (height + label_height);
        ensure!(
            total_width <= 8192
                && total_height <= 8192
                && u64::from(total_width) * u64::from(total_height) <= limits.max_pixels,
            "Contact sheet exceeds the image dimension or pixel limit; use fewer frames, more columns or smaller frame_width"
        );
        Ok(Self {
            image: RgbImage::from_pixel(total_width, total_height, Rgb([24, 24, 24])),
            columns,
            cell_width: width,
            thumb_height: height,
            label_height,
        })
    }
    fn place(&mut self, index: usize, image: &RgbImage, label: &str) -> Value {
        let x = index as u32 % self.columns * self.cell_width;
        let y = index as u32 / self.columns * (self.thumb_height + self.label_height);
        let factor = (self.cell_width as f64 / image.width() as f64)
            .min(self.thumb_height as f64 / image.height() as f64);
        let width = (image.width() as f64 * factor).round().max(1.0) as u32;
        let height = (image.height() as f64 * factor).round().max(1.0) as u32;
        let scaled = imageops::resize(image, width, height, FilterType::Triangle);
        let left = x + (self.cell_width - width) / 2;
        let top = y + (self.thumb_height - height) / 2;
        imageops::replace(&mut self.image, &scaled, left.into(), top.into());
        if self.label_height > 0 {
            label_text(
                &mut self.image,
                label,
                x,
                y + self.thumb_height,
                self.cell_width,
            );
        }
        json!({"x":left,"y":top,"width":width,"height":height})
    }
}
fn sheet(
    ffmpeg: &Path,
    video: &Video,
    a: &Value,
    cancel: &AtomicBool,
    limits: &Limits,
    deadline: Instant,
) -> Result<Output> {
    let start = a["start_seconds"].as_f64().unwrap_or(0.0);
    let end = a["end_seconds"].as_f64().unwrap_or(video.duration);
    ensure!(
        start >= 0.0 && start < end && end <= video.duration,
        "Require 0 <= start_seconds < end_seconds <= media duration ({})",
        video.duration
    );
    let count = a["max_frames"].as_u64().unwrap_or(24) as usize;
    ensure!(
        (1..=limits.contact_frames).contains(&count),
        "max_frames must be 1–{} under the configured inspection limit",
        limits.contact_frames
    );
    let width = a["frame_width"].as_u64().unwrap_or(480) as u32;
    ensure!(
        width <= limits.max_width,
        "frame_width exceeds configured inspection limit"
    );
    let columns = a["columns"].as_u64().unwrap_or(4) as u32;
    let labels = a["include_timestamps"].as_bool().unwrap_or(true);
    let sampling = a["sampling"].as_str().unwrap_or("interval");
    let (times, truncated, fallback, candidates) = match sampling {
        "uniform" => (uniform(start, end, count), false, false, count as u64),
        "scene" => {
            let cuts = detect_scenes(
                ffmpeg,
                video,
                SceneAnalysis {
                    start,
                    end,
                    threshold: a["threshold"].as_f64().unwrap_or(0.3),
                    min_seconds: 0.25,
                },
                cancel,
                deadline,
            )?;
            let (times, truncated, fallback) = scene_samples(start, end, &cuts, count);
            let candidates = if fallback {
                times.len() as u64
            } else {
                cuts.iter()
                    .filter(|cut| **cut > start && **cut < end)
                    .count() as u64
                    + 1
            };
            (times, truncated, fallback, candidates)
        }
        _ => {
            let step = a["interval_seconds"].as_f64().unwrap_or(10.0);
            ensure!(step > 0.0, "interval_seconds must be positive");
            let (times, truncated, candidates) = interval(start, end, step, count);
            (times, truncated, false, candidates)
        }
    };
    validate_times(&times, video.duration)?;
    render_sheet(
        Decode {
            ffmpeg,
            video,
            cancel,
            limits,
            deadline,
        },
        &times,
        width,
        columns,
        labels,
        json!({"duration_seconds":video.duration,"start_seconds":start,"end_seconds":end,"sampling":sampling,"effective_sampling":if fallback{"uniform"}else{sampling},"scene_fallback":fallback,"candidate_count":candidates,"truncated":truncated,"modified":truncated}),
    )
}
fn render_sheet(
    context: Decode<'_>,
    times: &[f64],
    width: u32,
    columns: u32,
    labels: bool,
    mut metadata: Value,
) -> Result<Output> {
    let Decode {
        ffmpeg,
        video,
        cancel,
        limits,
        deadline,
    } = context;
    let mut grid = Grid::new(
        times.len(),
        columns,
        width,
        video.display_aspect,
        labels,
        limits,
    )?;
    let mut frames=times.iter().enumerate().map(|(index,time)|json!({"index":index,"timestamp_seconds":time,"error":"No decodable frame at or after this timestamp"})).collect::<Vec<_>>();
    let result = extract(
        Decode {
            ffmpeg,
            video,
            cancel,
            limits,
            deadline,
        },
        times,
        width,
        true,
        |index, stamp, image| {
            let label = timecode(stamp.seconds);
            let rect = grid.place(index, image, &label);
            let mut meta = frame_metadata(index, times[index], stamp);
            meta["tile"] = rect;
            meta["image_index"] = json!(0);
            meta["content_index"] = json!(2);
            frames[index] = meta;
            Ok(())
        },
    );
    check(cancel, deadline)?;
    let actual = frames.iter().filter(|f| f.get("error").is_none()).count();
    ensure!(
        actual > 0,
        "No contact-sheet frames could be decoded: {}",
        result
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_else(|| "no matching frames".into())
    );
    let data = encode(&grid.image, "jpeg", 85)?;
    payload_size(data.len(), limits)?;
    Ok(Output {
        metadata: {
            metadata["frame_count"] = json!(actual);
            metadata["sample_count"] = json!(times.len());
            metadata["frames"] = json!(frames);
            metadata["partial"] = json!(actual != times.len());
            metadata["width"] = json!(grid.image.width());
            metadata["height"] = json!(grid.image.height());
            metadata["columns"] = json!(columns);
            metadata["include_timestamps"] = json!(labels);
            metadata["decoder_error"] = json!(result.err().map(|e| format!("{e:#}")));
            metadata
        },
        images: vec![Visual {
            data,
            mime: "image/jpeg",
            description: format!(
                "Contact sheet: {actual} frames, {}–{}",
                timecode(times[0]),
                timecode(*times.last().unwrap())
            ),
        }],
    })
}
fn timeline_scenes(
    ffmpeg: &Path,
    video: &Video,
    a: &Value,
    cancel: &AtomicBool,
    limits: &Limits,
    deadline: Instant,
) -> Result<Output> {
    let start = a["start_seconds"].as_f64().unwrap_or(0.0);
    let end = a["end_seconds"].as_f64().unwrap_or(video.duration);
    ensure!(
        start >= 0.0 && start < end && end <= video.duration,
        "Require 0 <= start_seconds < end_seconds <= media duration ({})",
        video.duration
    );
    let threshold = a["threshold"].as_f64().unwrap_or(0.35);
    let minimum = a["min_scene_seconds"].as_f64().unwrap_or(0.25);
    let max = a["max_scenes"].as_u64().unwrap_or(100) as usize;
    let thumbnails = a["include_thumbnails"].as_bool().unwrap_or(false);
    let width = a["frame_width"].as_u64().unwrap_or(240) as u32;
    let columns = a["columns"].as_u64().unwrap_or(4) as u32;
    ensure!(
        !thumbnails || width <= limits.max_width,
        "Scene thumbnail width exceeds configured limit"
    );
    let cuts = detect_scenes(
        ffmpeg,
        video,
        SceneAnalysis {
            start,
            end,
            threshold,
            min_seconds: minimum,
        },
        cancel,
        deadline,
    )?;
    let mut bounds = vec![start];
    bounds.extend(cuts);
    bounds.push(end);
    let representatives = bounds
        .windows(2)
        .map(|b| (b[0] + b[1]) * 0.5)
        .collect::<Vec<_>>();
    let indices = scene_indices(start, end, &representatives, max);
    ensure!(
        !thumbnails || indices.len() <= limits.contact_frames,
        "Scene thumbnails exceed the configured contact-sheet limit ({}); reduce max_scenes or set include_thumbnails=false",
        limits.contact_frames
    );
    let mut scenes=indices.iter().map(|i|json!({"index":i,"start_seconds":bounds[*i],"end_seconds":bounds[*i+1],"representative_seconds":representatives[*i]})).collect::<Vec<_>>();
    let analysis = json!({"method":"ffmpeg_scene_score","analysis_width":320,"threshold":threshold,"min_scene_seconds":minimum,"sampling":"native_pts","approximate":true});
    let metadata = json!({"duration_seconds":video.duration,"start_seconds":start,"end_seconds":end,"scene_count":scenes.len(),"total_scene_count":representatives.len(),"truncated":scenes.len()<representatives.len(),"modified":scenes.len()<representatives.len(),"threshold":threshold,"analysis":analysis,"scenes":scenes,"include_thumbnails":thumbnails});
    if !thumbnails {
        return Ok(Output {
            metadata,
            images: Vec::new(),
        });
    }
    let times = indices
        .iter()
        .map(|i| representatives[*i])
        .collect::<Vec<_>>();
    let mut output = render_sheet(
        Decode {
            ffmpeg,
            video,
            cancel,
            limits,
            deadline,
        },
        &times,
        width,
        columns,
        true,
        metadata,
    )?;
    for (i, scene) in scenes.iter_mut().enumerate() {
        let frame = &mut output.metadata["frames"][i];
        frame["scene_index"] = scene["index"].clone();
        scene["representative_actual_seconds"] = frame["actual_timestamp_seconds"].clone();
        scene["thumbnail_index"] = json!(i);
        scene["image_index"] = json!(0);
        scene["content_index"] = json!(2);
        if let Some(error) = frame.get("error") {
            scene["thumbnail_error"] = error.clone();
        }
    }
    output.metadata["scenes"] = json!(scenes);
    Ok(output)
}
// A small numeric bitmap font keeps headless timestamp rendering independent of
// GUI/font discovery and makes identical requests reproducible across platforms.
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        ':' => [0, 4, 4, 0, 4, 4, 0],
        '.' => [0, 0, 0, 0, 0, 6, 6],
        _ => [0; 7],
    }
}
fn label_text(image: &mut RgbImage, text: &str, x: u32, y: u32, width: u32) {
    let scale = if width >= 240 { 2 } else { 1 };
    let text_width = text.chars().count() as u32 * 6 * scale;
    let left = x + (width.saturating_sub(text_width)) / 2;
    let top = y + (28 - 7 * scale) / 2;
    for (n, c) in text.chars().enumerate() {
        for (row, bits) in glyph(c).iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = left + n as u32 * 6 * scale + column * scale + sx;
                            let py = top + row as u32 * scale + sy;
                            if px < image.width() && py < image.height() {
                                image.put_pixel(px, py, Rgb([220, 220, 220]));
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    #[test]
    fn sampling_and_scene_fallback_are_deterministic() {
        assert_eq!(
            interval(900.0, 920.0, 5.0, 24),
            (vec![900.0, 905.0, 910.0, 915.0], false, 4)
        );
        assert_eq!(interval(0.0, 20.0, 3.0, 3), (vec![0.0, 3.0, 6.0], true, 7));
        assert_eq!(uniform(0.0, 12.0, 3), vec![2.0, 6.0, 10.0]);
        assert_eq!(
            scene_samples(0.0, 12.0, &[], 3),
            (vec![2.0, 6.0, 10.0], false, true)
        );
        assert_eq!(
            scene_samples(0.0, 12.0, &[4.0, 8.0], 3),
            (vec![2.0, 6.0, 10.0], false, false)
        );
        let selected = scene_samples(0.0, 12.0, &[2.0, 4.0, 6.0, 8.0, 10.0], 3);
        assert!(selected.1 && !selected.2);
        assert_eq!(selected.0.len(), 3);
        assert_eq!(
            selected,
            scene_samples(0.0, 12.0, &[2.0, 4.0, 6.0, 8.0, 10.0], 3)
        );
    }
    #[test]
    fn sparse_overviews_seek_and_dense_batches_share_a_decoder() {
        let groups = seek_groups(&[100.0, 1.0, 0.0, 21.0, 20.0, 1.0]);
        assert_eq!(groups, vec![vec![2, 1, 5], vec![4, 3], vec![0]]);
        assert_eq!(seek_groups(&[0.0, 2.5, 5.0]), vec![vec![0, 1, 2]]);
    }
    #[test]
    fn grid_and_timestamp_labels_match_metadata_units() {
        assert_eq!(timecode(900.125), "00:15:00.125");
        assert_eq!(timecode(3599.9996), "01:00:00.000");
        let mut grid = Grid::new(5, 2, 128, 4.0 / 3.0, true, &Limits::default()).unwrap();
        assert_eq!(grid.image.dimensions(), (256, 372));
        let rect = grid.place(
            0,
            &RgbImage::from_pixel(128, 96, Rgb([10, 20, 30])),
            "00:15:00.125",
        );
        assert_eq!(rect, json!({"x":0,"y":0,"width":128,"height":96}));
        assert!(
            imageops::crop_imm(&grid.image, 0, 96, 128, 28)
                .pixels()
                .any(|(_, _, p)| p.0 == [220, 220, 220])
        );
        assert_eq!(
            Grid::new(5, 2, 128, 4.0 / 3.0, false, &Limits::default())
                .unwrap()
                .image
                .dimensions(),
            (256, 288)
        );
        assert!(Grid::new(64, 1, 1920, 1.0, true, &Limits::default()).is_err());
    }
    #[test]
    fn arguments_and_timestamps_are_bounded_before_decode() {
        for arguments in [
            json!({"timestamps_seconds":[]}),
            json!({"timestamps_seconds":["1"]}),
            json!({"timestamps_seconds":vec![0;17]}),
            json!({"timestamps_seconds":[0],"quality":101}),
        ] {
            assert!(
                super::super::tools::validate(&Request {
                    name: "video_extract_frames".into(),
                    arguments
                })
                .is_err()
            );
        }
        assert!(validate_times(&[f64::NAN], 3.0).is_err());
        assert!(validate_times(&[3.0], 3.0).is_err());
        assert!(validate_times(&[-0.1], 3.0).is_err());
        assert!(payload_size(10 * 1024 * 1024, &Limits::default()).is_err());
    }
    #[test]
    fn configured_frame_caps_are_checked_before_decoder_start() {
        let video = Video {
            path: PathBuf::from("unused"),
            duration: 3.0,
            display_aspect: 1.0,
        };
        let limits = Limits {
            contact_frames: 2,
            batch_frames: 1,
            ..Default::default()
        };
        for request in [
            Request {
                name: "video_extract_frames".into(),
                arguments: json!({"timestamps_seconds":[0,1]}),
            },
            Request {
                name: "video_contact_sheet".into(),
                arguments: json!({"max_frames":3}),
            },
        ] {
            let error = run(
                Path::new("missing-decoder"),
                &video,
                &request,
                &AtomicBool::new(false),
                &limits,
            )
            .err()
            .unwrap()
            .to_string();
            assert!(error.contains("configured"), "{error}");
        }
    }
    #[test]
    fn ppm_and_native_pts_are_independent_of_nominal_fps() {
        let (tx, rx) = bounded(2);
        let mut parse = stamp_parser(tx);
        parse("[Parsed_showinfo_0] config in time_base: 1/90000, frame_rate: 24/1");
        parse("[Parsed_showinfo_0] n: 0 pts: 11250 pts_time:0.125 duration:1");
        let stamp = rx.recv().unwrap();
        assert_eq!(stamp.seconds, 0.125);
        assert_eq!(stamp.pts, 11250);
        let mut ppm_data = Cursor::new(b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06");
        assert_eq!(
            ppm(&mut ppm_data, &Limits::default())
                .unwrap()
                .unwrap()
                .get_pixel(1, 0)
                .0,
            [4, 5, 6]
        );
        assert!(
            ppm(
                &mut Cursor::new(b"P6\n99999 99999\n255\n"),
                &Limits::default()
            )
            .is_err()
        );
    }
    #[cfg(unix)]
    fn script(scratch: &crate::export::Scratch, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch.path.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[cfg(unix)]
    #[test]
    fn partial_decode_returns_successful_images_and_per_frame_errors() {
        let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 98601).unwrap();
        let ffmpeg = script(
            &scratch,
            "partial",
            "printf '[showinfo] config in time_base: 1/1000, frame_rate: 8/1\\n[showinfo] n: 0 pts: 0 pts_time:0\\n' >&2\nprintf 'P6\\n2 2\\n255\\nabcdefghijkl'\nexit 1",
        );
        let video = Video {
            path: scratch.path.join("unused.mkv"),
            duration: 3.0,
            display_aspect: 1.0,
        };
        let result = run(
            &ffmpeg,
            &video,
            &Request {
                name: "video_extract_frames".into(),
                arguments: json!({"timestamps_seconds":[0,1],"format":"png"}),
            },
            &AtomicBool::new(false),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.metadata["frames"][0]["image_index"], 0);
        assert!(result.metadata["frames"][1]["error"].is_string());
        assert_eq!(result.metadata["partial"], true);
        assert!(result.images[0].data.starts_with(b"\x89PNG"));
    }
    #[cfg(unix)]
    #[test]
    fn cancellation_and_timeout_reap_silent_decoders() {
        let scratch = crate::export::Scratch::new(&std::env::temp_dir(), 98602).unwrap();
        let ffmpeg = script(
            &scratch,
            "silent",
            "printf '%s' \"$$\" > \"${0}.pid\"\nwhile :; do :; done",
        );
        let video = Video {
            path: scratch.path.join("unused.mkv"),
            duration: 3.0,
            display_aspect: 1.0,
        };
        let request = Request {
            name: "video_extract_frames".into(),
            arguments: json!({"timestamps_seconds":[0]}),
        };
        for cancelled in [false, true] {
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let token = cancel.clone();
            let thread = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                if cancelled {
                    token.store(true, Ordering::Relaxed);
                }
            });
            let limits = Limits {
                timeout: Duration::from_millis(180),
                ..Default::default()
            };
            let before = Instant::now();
            assert!(run(&ffmpeg, &video, &request, &cancel, &limits).is_err());
            assert!(before.elapsed() < Duration::from_secs(2));
            thread.join().unwrap();
            #[cfg(target_os = "linux")]
            {
                let pid = std::fs::read_to_string(scratch.path.join("silent.pid")).unwrap();
                assert!(!Path::new(&format!("/proc/{pid}")).exists());
            }
        }
    }
}
