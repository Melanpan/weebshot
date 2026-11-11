use std::process::{Command, Output};

use serde_json::Value;

use super::*;
use crate::export::{ClipFormat, Crop, find_tool, run_job, tests::job};

const CROP: Crop = Crop {
    x: 16,
    y: 12,
    width: 112,
    height: 80,
};

fn checked(out: Output) -> Vec<u8> {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn fixture(dir: &Path) -> PathBuf {
    let subtitles = dir.join("source.srt");
    std::fs::write(&subtitles, "1\n00:00:00,500 --> 00:00:01,500\nAnime4K\n").unwrap();
    let input = dir.join("source.mkv");
    checked(
        Command::new(find_tool("ffmpeg"))
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=160x120:r=24:d=2",
            ])
            .args(["-i"])
            .arg(subtitles)
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=f=440:r=48000:d=2",
                "-map",
                "0:v",
                "-map",
                "1:s",
                "-map",
                "2:a",
                "-vf",
                "select=not(eq(mod(n\\,5)\\,1)),setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                "-fps_mode",
                "vfr",
                "-c:v",
                "ffv1",
                "-c:a",
                "pcm_s16le",
                "-c:s",
                "srt",
            ])
            .arg(&input)
            .output()
            .unwrap(),
    );
    input
}

fn options() -> Options {
    Options {
        enabled: true,
        speed: Speed::Fast,
        device: Device::Cpu,
        ..Options::default()
    }
}

fn probe(path: &Path, args: &[&str]) -> Value {
    let data = checked(
        Command::new(find_tool("ffprobe"))
            .args(["-v", "error", "-of", "json"])
            .args(args)
            .arg(path)
            .output()
            .unwrap(),
    );
    serde_json::from_slice(&data).unwrap()
}

fn animation_webp(path: &Path) -> (u32, u32, u16, Vec<u32>) {
    let data = std::fs::read(path).unwrap();
    assert_eq!(&data[..4], b"RIFF");
    assert_eq!(&data[8..12], b"WEBP");
    let u24 = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], 0]);
    let (mut width, mut height, mut loop_count) = (0, 0, u16::MAX);
    let mut frame_durations = Vec::new();
    let mut chunks = &data[12..];
    while let Some(header) = chunks.get(..8) {
        let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let payload = &chunks[8..8 + size];
        match &header[..4] {
            b"VP8X" => {
                width = 1 + u24(&payload[4..]);
                height = 1 + u24(&payload[7..]);
            }
            b"ANIM" => loop_count = u16::from_le_bytes(payload[4..6].try_into().unwrap()),
            b"ANMF" => frame_durations.push(u24(&payload[12..])),
            _ => {}
        }
        chunks = &chunks[8 + size + size % 2..];
    }
    (width, height, loop_count, frame_durations)
}

/// Run with the native helper, ffmpeg and ffprobe on PATH.
#[test]
#[ignore = "requires a built weebshot-anime4k helper and FFmpeg"]
fn anime4k_clip_formats() {
    let scratch = Scratch::new(&std::env::temp_dir(), 600).unwrap();
    let input = fixture(&scratch.path);
    for format in ClipFormat::ALL {
        for burn in [false, true] {
            let name = format!("clip-{burn}.{}", format.ext());
            let mut clip = job(&input, format, burn, &name);
            clip.anime4k = options();
            clip.start = 0.210;
            clip.end = 1.460;
            clip.crop = Some(CROP);
            clip.src_height = Some(120);
            clip.src_fps = Some(24.0);
            clip.max_height = Some(if burn { 96 } else { 200 });
            clip.max_fps = burn.then_some(12);
            clip.fps = 10;
            clip.anim.dedupe = false;
            clip.loop_count = if burn { 2 } else { 1 };
            let tools = super::super::tests::tools();
            run_job(&tools, 601, &clip, &AtomicBool::new(false), |_, _, _| {})
                .unwrap_or_else(|error| panic!("{name}: {error:#}"));
            let expected = if burn { (134, 96) } else { (224, 160) };
            if format == ClipFormat::Webp {
                let (width, height, loops, durations) = animation_webp(&clip.output);
                assert_eq!((width, height), expected);
                assert_eq!(loops, 0);
                assert!(durations.len() > 5);
                assert!(durations.iter().all(|&duration| duration == 100));
                assert!(
                    (if burn { 2300u32..=2700 } else { 1100..=1300 })
                        .contains(&durations.iter().sum::<u32>())
                );
                continue;
            }

            let streams = probe(&clip.output, &["-show_streams", "-show_format"]);
            let video = &streams["streams"][0];
            assert_eq!(video["width"], expected.0, "{name}");
            assert_eq!(video["height"], expected.1, "{name}");
            if format.has_audio() {
                assert_eq!(streams["streams"][1]["codec_type"], "audio");
                assert_eq!(video["color_space"], "bt709");
                assert_eq!(video["color_primaries"], "bt709");
                assert_eq!(video["color_transfer"], "bt709");
            }
            let duration = streams["format"]["duration"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap();
            assert!(
                (if burn { 2.30..=2.70 } else { 1.10..=1.35 }).contains(&duration),
                "{name}: {duration}"
            );
            let frames = probe(
                &clip.output,
                &[
                    "-select_streams",
                    "v:0",
                    "-show_frames",
                    "-show_entries",
                    "frame=best_effort_timestamp_time",
                ],
            );
            let pts: Vec<_> = frames["frames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|frame| {
                    frame["best_effort_timestamp_time"]
                        .as_str()
                        .unwrap()
                        .parse::<f64>()
                        .unwrap()
                })
                .collect();
            if burn || format.is_animation() {
                let min_interval = if format.is_animation() {
                    0.1
                } else {
                    1.0 / 12.0
                };
                assert!(
                    pts.windows(2)
                        .all(|pair| pair[1] - pair[0] >= min_interval - 0.002),
                    "{name}: {pts:?}"
                );
            } else {
                let audio = streams["streams"][1]["start_time"]
                    .as_str()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap();
                let source_pts = probe(
                    &input,
                    &[
                        "-select_streams",
                        "v:0",
                        "-show_packets",
                        "-show_entries",
                        "packet=pts_time",
                    ],
                );
                let expected: Vec<_> = source_pts["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|packet| packet["pts_time"].as_str().unwrap().parse::<f64>().unwrap())
                    .filter(|&pts| pts >= clip.start && pts < clip.end)
                    .map(|pts| pts - clip.start)
                    .collect();
                assert_eq!(pts.len(), expected.len(), "{name}");
                for (actual, expected) in pts.iter().zip(expected) {
                    assert!(
                        (actual - audio - expected).abs() < 0.011,
                        "{name}: {actual} - {audio} vs {expected}"
                    );
                }
            }
        }
    }
    for format in [ClipFormat::Mp4, ClipFormat::Webm] {
        let mut clip = job(&input, format, true, &format!("target.{}", format.ext()));
        clip.anime4k = options();
        clip.start = 0.21;
        clip.end = 1.46;
        clip.target_mb = Some(0.25);
        clip.max_height = Some(144);
        let tools = super::super::tests::tools();
        run_job(&tools, 602, &clip, &AtomicBool::new(false), |_, _, _| {}).unwrap();
        assert!(std::fs::metadata(clip.output).unwrap().len() <= 256 * 1024);
    }
}

#[test]
#[ignore = "requires a built weebshot-anime4k helper and FFmpeg"]
fn anime4k_stills_models_and_cancelled_retry() {
    let scratch = Scratch::new(&std::env::temp_dir(), 610).unwrap();
    let input = fixture(&scratch.path);
    let tools = super::super::tests::tools();
    let cancel = AtomicBool::new(false);
    let subbed = StillJob {
        input: input.clone(),
        time: 0.85,
        fps: 24.0,
        crop: Some(CROP),
        subs: Some(SubSource::Text {
            path: input.clone(),
            stream: 0,
        }),
        output: PathBuf::new(),
    };
    let mut still = Job {
        input: input.clone(),
        frame: Frame::Extract(subbed.clone()),
        options: options(),
        output: None,
    };
    let pixels = |still: &Job| {
        still
            .run(&tools, 611, &cancel, |_, _, _| {})
            .unwrap()
            .unwrap()
    };
    let subbed_pixels = pixels(&still);
    assert_eq!(subbed_pixels.dimensions(), (224, 160));
    still.frame = Frame::Extract(StillJob {
        subs: None,
        ..subbed.clone()
    });
    let neutral_pixels = pixels(&still);
    assert_ne!(subbed_pixels, neutral_pixels);
    still.options.model = Model::ArNet;
    still.options.variant = Variant::LinesDenoise;
    assert_ne!(neutral_pixels, pixels(&still));

    let png = scratch.path.join("source.png");
    super::super::run_still(
        &tools.ffmpeg,
        &StillJob {
            crop: None,
            output: png.clone(),
            ..subbed
        },
    )
    .unwrap();
    still.frame = Frame::Captured(std::fs::read(png).unwrap().into());
    assert_eq!(pixels(&still).dimensions(), (320, 240));
    for extension in ["png", "jpg", "webp"] {
        let output = scratch.path.join(format!("still.{extension}"));
        still.output = Some(output.clone());
        still.run(&tools, 611, &cancel, |_, _, _| {}).unwrap();
        let frame = probe(&output, &["-show_streams"]);
        assert_eq!(frame["streams"][0]["width"], 320);
        assert_eq!(frame["streams"][0]["height"], 240);
    }

    let mut clip = job(&input, ClipFormat::Mp4, true, "cancel-retry.mp4");
    clip.anime4k = options();
    clip.start = 0.21;
    clip.end = 1.46;
    let error = run_job(&tools, 612, &clip, &cancel, |_, fraction, stage| {
        if stage == "Upscaling (Anime4K CPU)" && fraction.is_some_and(|f| f > 0.0) {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "cancelled");
    assert!(!clip.output.exists());
    let prefix = format!(".weebshot-{}-612-", std::process::id());
    for dir in [&std::env::temp_dir(), clip.output.parent().unwrap()] {
        assert!(
            !std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        );
    }
    cancel.store(false, Ordering::Relaxed);
    run_job(&tools, 612, &clip, &cancel, |_, _, _| {}).unwrap();
    assert!(clip.output.is_file());
}
