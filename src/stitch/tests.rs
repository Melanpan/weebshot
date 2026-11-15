use super::*;
use image::{Luma, Rgb};
use motion::Motion;
fn estimate(a: &GrayImage, b: &GrayImage) -> Option<Motion> {
    motion::estimate(a, b, &Default::default())
}

fn artwork(w: u32, h: u32) -> RgbImage {
    // Reproducible illustration-like edges, gradients and fine texture.
    let mut seed = 0x12345678u32;
    RgbImage::from_fn(w, h, |x, y| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let a = ((x * 7 + y * 3 + (x / 17) * 61 + (y / 13) * 47) % 180) as u8;
        let b = if (x as i32 - 130).pow(2) + (y as i32 - 110).pow(2) < 2500 {
            45
        } else {
            0
        };
        let v = a.saturating_add(b).saturating_add((seed % 24) as u8);
        Rgb([v, v.saturating_add(12), v.saturating_sub(10)])
    })
}
fn crop(src: &RgbImage, x: u32, y: u32) -> RgbImage {
    imageops::crop_imm(src, x, y, 128, 96).to_image()
}
fn gray(src: &RgbImage) -> GrayImage {
    imageops::grayscale(src)
}
fn pan(
    path: &[(u32, u32)],
    modify: impl Fn(usize, &mut RgbImage),
) -> (RgbImage, Vec<RgbImage>, Vec<GrayImage>) {
    let source = artwork(384, 320);
    let frames: Vec<_> = path
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let mut f = crop(&source, x, y);
            modify(i, &mut f);
            f
        })
        .collect();
    let grays = frames.iter().map(gray).collect();
    (source, frames, grays)
}
fn check(path: &[(u32, u32)]) {
    let started = Instant::now();
    let (src, frames, grays) = pan(path, |_, _| {});
    let motions: Vec<_> = grays.windows(2).map(|p| estimate(&p[0], &p[1])).collect();
    let positions = positions(
        &grays,
        &motions,
        0,
        grays.len() - 1,
        &AtomicBool::new(false),
        &Options::default(),
    )
    .unwrap();
    for (p, &(x, y)) in positions.iter().zip(path) {
        assert!((p.x - (x as i32 - path[0].0 as i32)).abs() <= 1, "x: {p:?}");
        assert!((p.y - (y as i32 - path[0].1 as i32)).abs() <= 1, "y: {p:?}");
    }
    let (min, size) = bounds(&positions, (128, 96)).unwrap();
    let mut composer = compose::Composer::new(size);
    for (frame, p) in frames.iter().zip(&positions) {
        composer
            .push(
                frame,
                None,
                ((p.x - min.x) as u32, (p.y - min.y) as u32),
                &AtomicBool::new(false),
            )
            .unwrap();
    }
    let out = composer.finish();
    let origin = (
        path.iter().map(|p| p.0).min().unwrap(),
        path.iter().map(|p| p.1).min().unwrap(),
    );
    let mut error = 0u64;
    let mut coverage = 0;
    for (x, y, p) in out.enumerate_pixels() {
        if p[3] > 0 {
            coverage += 1;
            let original = src.get_pixel(x + origin.0, y + origin.1);
            for c in 0..3 {
                error += p[c].abs_diff(original[c]) as u64;
            }
        }
    }
    assert_eq!(error, 0, "Source pixels must be preserved exactly");
    let expected = (
        128 + path.iter().map(|p| p.0).max().unwrap() - origin.0,
        96 + path.iter().map(|p| p.1).max().unwrap() - origin.1,
    );
    assert_eq!(size, expected);
    assert!(coverage as f64 / (size.0 * size.1) as f64 > 0.85);
    eprintln!(
        "{} frames, {:?}, covered {coverage} pixels, error {error}, {:?}, composition buffers {} bytes",
        frames.len(),
        size,
        started.elapsed(),
        size.0 as usize * size.1 as usize * 5
    );
}
#[test]
fn vertical_pan() {
    check(&[(10, 10), (10, 18), (10, 26), (10, 34), (10, 42), (10, 50)]);
}
#[test]
fn horizontal_pan() {
    check(&[(10, 10), (22, 10), (34, 10), (46, 10), (58, 10)]);
}
#[test]
fn diagonal_pan() {
    check(&[(10, 10), (16, 14), (22, 18), (28, 22), (34, 26)]);
}
#[test]
fn reversed_pan() {
    check(&[(60, 60), (48, 60), (36, 60), (24, 60), (12, 60)]);
}
#[test]
fn duplicates_and_pause() {
    check(&[(10, 10), (10, 18), (10, 18), (10, 18), (10, 26), (10, 34)]);
}
#[test]
fn noise_and_foreground() {
    for foreground in [false, true] {
        let (_, _, frames) = pan(&[(10, 10), (10, 20)], |i, f| {
            if i == 1 {
                for (x, y, p) in f.enumerate_pixels_mut() {
                    if foreground && x < 30 && y < 25 {
                        *p = Rgb([240, 20, 60]);
                    } else if !foreground {
                        for c in 0..3 {
                            p[c] = p[c].saturating_add(((x * 17 + y * 13) % 7) as u8);
                        }
                    }
                }
            }
        });
        let m = estimate(&frames[0], &frames[1]).unwrap();
        assert!(m.reliable(&Default::default()), "{m:?}");
        assert_eq!((m.dx, m.dy), (0, 10));
    }
}
#[test]
fn low_detail_rejected() {
    assert!(
        estimate(
            &GrayImage::from_pixel(128, 96, Luma([80])),
            &GrayImage::from_pixel(128, 96, Luma([90]))
        )
        .is_none()
    );
}
#[test]
fn no_movement() {
    let (_, _, f) = pan(&[(10, 10), (10, 10)], |_, _| {});
    let m = estimate(&f[0], &f[1]).unwrap();
    assert!(m.reliable(&Default::default()));
    assert!(!m.moving());
    assert!(region(&[Some(m)], 0, &Options::default(), 8.0).is_err());
}
fn moving(dx: i32, dy: i32) -> Option<Motion> {
    Some(Motion {
        dx,
        dy,
        error: 0.0,
        support: 1.0,
    })
}
#[test]
fn boundaries_allow_short_pause_but_stop_at_cut_and_reversal() {
    let motions = vec![
        None,
        moving(0, 4),
        moving(0, 4),
        moving(0, 0),
        moving(0, 0),
        moving(0, 4),
        moving(0, 4),
        None,
        moving(0, -4),
    ];
    assert_eq!(
        region(&motions, 3, &Options::default(), 8.0).unwrap(),
        (1, 7)
    );
    // A playhead on an unmatched pair (the first frame of a clip whose camera
    // already rolls) still finds the pan that starts next to it.
    let unmatched_start = vec![None, moving(0, 4), moving(0, 4), moving(0, 4), None];
    for seed in 0..=4 {
        assert_eq!(
            region(&unmatched_start, seed, &Options::default(), 8.0).unwrap(),
            (1, 4),
            "seed {seed}"
        );
    }
    let reversed = vec![moving(3, 0), moving(3, 0), moving(-3, 0)];
    assert_eq!(
        region(&reversed, 1, &Options::default(), 8.0).unwrap(),
        (0, 2)
    );
}
#[test]
fn actual_scene_cut() {
    let (_, _, f) = pan(&[(10, 10), (10, 20)], |i, f| {
        if i == 1 {
            for (x, y, p) in f.enumerate_pixels_mut() {
                *p = Rgb([((x * 131 + y * 73) % 256) as u8, 0, 255]);
            }
        }
    });
    assert!(estimate(&f[0], &f[1]).is_none_or(|m| !m.reliable(&Default::default())));
}
#[test]
fn adaptive_selection_skips_holds() {
    let p = vec![
        Position { x: 0, y: 0 },
        Position { x: 0, y: 0 },
        Position { x: 0, y: 2 },
        Position { x: 0, y: 10 },
        Position { x: 0, y: 20 },
    ];
    assert_eq!(select(&p, (128, 96), &Options::default()), vec![0, 3, 4]);
}
#[test]
fn huge_canvas_rejected_before_allocation() {
    assert!(
        bounds(
            &[Position::default(), Position { x: 16000, y: 16000 }],
            (1920, 1080)
        )
        .is_err()
    );
    assert!(
        bounds(
            &[Position::default(), Position { x: 0, y: 10000 }],
            (3840, 2160)
        )
        .is_err()
    );
}
#[test]
fn cancellation_during_composition() {
    let mut composer = compose::Composer::new((128, 96));
    assert!(
        composer
            .push(&artwork(128, 96), None, (0, 0), &AtomicBool::new(true))
            .is_err()
    );
}

/// A real lossless video fixture generated from deterministic artwork. Exercises the
/// installed FFmpeg binary, sampling timestamp consistency, native refinement, PNG
/// dimensions, automatic discovery and cancellation. Run on CI with FFmpeg installed.
#[test]
fn ffmpeg_video_round_trip() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("FFmpeg unavailable; video fixture test skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("weebshot-stitch-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let source = artwork(384, 320);
    for i in 0..24 {
        crop(&source, 20, 10 + i * 3)
            .save(dir.join(format!("{i:03}.png")))
            .unwrap();
    }
    let video = dir.join("pan.mkv");
    let status = std::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-framerate",
            "8",
            "-i",
        ])
        .arg(dir.join("%03d.png"))
        .args(["-c:v", "ffv1"])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());
    let job = Job {
        input: video,
        playhead: 1.5,
        duration: 3.0,
        range: Some((0.0, 3.0)),
        source_fps: 8.0,
        options: Options::default(),
    };
    // Count actual decoded frames before the selector. The former output-only
    // duration limit decoded to EOF once no later selected frames could advance
    // the output clock. A short request must not decode the remaining fixture.
    let mut bounded = decode::selection_args(&job.input, 0.0, 1.0, 8.0, &[0, 1]);
    let log = bounded.iter().position(|a| a == "-loglevel").unwrap() + 1;
    bounded[log] = "info".into();
    let vf = bounded.iter().position(|a| a == "-vf").unwrap() + 1;
    bounded[vf] = bounded[vf].replacen(",select=", ",showinfo,select=", 1);
    let decoded = std::process::Command::new(&ffmpeg)
        .args(&bounded)
        .output()
        .unwrap();
    assert!(decoded.status.success());
    let stderr = String::from_utf8_lossy(&decoded.stderr);
    let last = stderr
        .lines()
        .filter_map(|line| {
            line.split("pts_time:")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
        })
        .fold(0.0f64, f64::max);
    assert!(
        last > 0.0 && last < 1.0,
        "Decoded beyond selected frames: {last}s"
    );

    // Native decode may seek past an unused prefix, but must keep exactly the
    // original FPS filter's frames for integer/fractional rates and seek times.
    for start in [0.0, 0.017] {
        for (fps, indices) in [
            (8.0, vec![18, 20, 22]),
            (16.0, vec![36, 40, 44]),
            (24000.0 / 1001.0, vec![54, 60, 66]),
        ] {
            let select = indices
                .iter()
                .map(|i| format!("eq(n\\,{i})"))
                .collect::<Vec<_>>()
                .join("+");
            let baseline = std::process::Command::new(&ffmpeg)
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-loglevel",
                    "error",
                    "-threads",
                    "2",
                    "-ss",
                    &format!("{start:.6}"),
                    "-i",
                ])
                .arg(&job.input)
                .args([
                    "-vf",
                    &format!("fps={fps}:start_time=0,select={select}"),
                    "-fps_mode",
                    "passthrough",
                    "-frames:v",
                    "3",
                    "-threads",
                    "1",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "pipe:1",
                ])
                .output()
                .unwrap();
            assert!(baseline.status.success());
            let mut actual = Vec::new();
            decode::selected_frames(
                &ffmpeg,
                &job.input,
                start,
                3.0,
                fps,
                &indices,
                (128, 96),
                &AtomicBool::new(false),
                |_, rgb| {
                    actual.extend_from_slice(rgb.as_raw());
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(
                actual, baseline.stdout,
                "FPS grid changed after seeking: start={start}, fps={fps}"
            );
        }
    }
    let out = run(&ffmpeg, 900000, &job, &AtomicBool::new(false), |_, _| {}).unwrap();
    assert_eq!(out.dimensions, (128, 165));
    let mut included = out.included.clone();
    included[0] = false;
    let edited = recompose(
        &ffmpeg,
        900030,
        out.recipe.clone(),
        included.clone(),
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert_eq!(edited.frames, out.frames - 1);
    assert!(edited.range.0 > out.range.0);
    assert_eq!(edited.recipe.frames.len(), out.recipe.frames.len());
    assert_eq!(edited.included, included);
    let restored = recompose(
        &ffmpeg,
        900031,
        edited.recipe.clone(),
        vec![true; included.len()],
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&restored.artifact.0).unwrap(),
        std::fs::read(&out.artifact.0).unwrap()
    );
    assert!(
        recompose(
            &ffmpeg,
            900032,
            out.recipe.clone(),
            vec![false; included.len()],
            &AtomicBool::new(false),
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(
        recompose(
            &ffmpeg,
            900033,
            out.recipe.clone(),
            out.included.clone(),
            &AtomicBool::new(true),
            |_, _| {}
        )
        .err()
        .unwrap()
        .to_string(),
        "cancelled"
    );

    let destination = dir.join("exported.png");
    crate::export::save_image_copy(&out.artifact.0, &destination, &job.input, 900020).unwrap();
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        std::fs::read(&out.artifact.0).unwrap()
    );
    assert!(
        crate::export::save_image_copy(&out.artifact.0, &job.input, &job.input, 900021).is_err()
    );
    let existing = std::fs::read(&destination).unwrap();
    assert!(
        crate::export::save_image_copy(&dir.join("missing.png"), &destination, &job.input, 900022)
            .is_err()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), existing);

    let saved = image::open(&out.artifact.0).unwrap().to_rgba8();
    for (x, y, p) in saved.enumerate_pixels() {
        assert_eq!(&p.0[..3], &source.get_pixel(x + 20, y + 10).0);
    }
    let auto = run(
        &ffmpeg,
        900001,
        &Job {
            range: None,
            ..job.clone()
        },
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert_eq!(auto.dimensions, out.dimensions);
    let cancel = AtomicBool::new(false);
    let result = run(&ffmpeg, 900002, &job, &cancel, |progress, _| {
        if progress > 0.18 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(result.err().unwrap().to_string(), "cancelled");
    assert!(
        !std::env::temp_dir()
            .join(format!("weebshot-stitch-{}-900002.png", std::process::id()))
            .exists()
    );

    let cancel = AtomicBool::new(false);
    let result = run(&ffmpeg, 900003, &job, &cancel, |progress, _| {
        if progress >= 0.7 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(result.err().unwrap().to_string(), "cancelled");
    assert!(
        !std::env::temp_dir()
            .join(format!("weebshot-stitch-{}-900003.png", std::process::id()))
            .exists()
    );

    // An explicit range across a cut keeps one coherent side and says so;
    // automatic discovery must stop before it.
    for i in 12..24 {
        let mut frame = crop(&source, 20, 10 + i * 3);
        imageops::invert(&mut frame);
        frame.save(dir.join(format!("{i:03}.png"))).unwrap();
    }
    let cut = dir.join("cut.mkv");
    assert!(
        std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                "8",
                "-i"
            ])
            .arg(dir.join("%03d.png"))
            .args(["-c:v", "ffv1"])
            .arg(&cut)
            .status()
            .unwrap()
            .success()
    );
    let cut_job = Job {
        input: cut,
        playhead: 0.5,
        ..job.clone()
    };
    let trimmed = run(
        &ffmpeg,
        900004,
        &cut_job,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert!(trimmed.range_trimmed);
    assert!(
        trimmed.warning.contains("marked range"),
        "{}",
        trimmed.warning
    );
    assert!(
        trimmed.range.1 < 1.5 || trimmed.range.0 >= 1.5,
        "stitched across the cut: {:?}",
        trimmed.range
    );
    assert_eq!(trimmed.dimensions, (128, 129));
    let before_cut = run(
        &ffmpeg,
        900005,
        &Job {
            range: None,
            ..cut_job
        },
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert_eq!(before_cut.dimensions, (128, 129));
    assert!(before_cut.range.1 < 1.5);
}

#[test]
fn ffmpeg_slow_pan_native_detail_and_pixel_aspect() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir =
        std::env::temp_dir().join(format!("weebshot-stitch-slow-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let source = artwork(1400, 900);
    for i in 0..24 {
        imageops::crop_imm(&source, 20, 10 + i, 1280, 720)
            .to_image()
            .save(dir.join(format!("{i:03}.png")))
            .unwrap();
    }
    let video = dir.join("slow.mkv");
    let status = std::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-framerate",
            "8",
            "-i",
        ])
        .arg(dir.join("%03d.png"))
        .args(["-vf", "setsar=4/3", "-c:v", "ffv1"])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());
    let job = Job {
        input: video,
        playhead: 1.5,
        duration: 3.0,
        range: None,
        source_fps: 8.0,
        options: Options::default(),
    };
    let out = run(&ffmpeg, 900010, &job, &AtomicBool::new(false), |_, _| {}).unwrap();
    assert_eq!(out.dimensions.0, 1280);
    assert_eq!(out.dimensions.1, 743);
    assert!((out.pixel_aspect - 4.0 / 3.0).abs() < 0.001);
    let bytes = std::fs::read(&out.artifact.0).unwrap();
    // Standard PNG densities are inverse to source pixel aspect.
    assert_eq!(png::ffmpeg_aspect(&bytes), (3, 4));
    let saved = image::load_from_memory(&bytes).unwrap().to_rgba8();
    for (x, y, p) in saved.enumerate_pixels() {
        assert_eq!(&p.0[..3], &source.get_pixel(x + 20, y + 10).0);
    }
    eprintln!(
        "slow native fixture: {:?}, {} frames, {:.2}s",
        out.dimensions, out.frames, out.elapsed
    );
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        eprintln!(
            "{}",
            status
                .lines()
                .find(|l| l.starts_with("VmHWM:"))
                .unwrap_or("")
        );
    }
}

#[cfg(unix)]
#[test]
fn cancellation_interrupts_silent_decoder() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("weebshot-silent-decoder-{}", std::process::id()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    std::fs::write(&path, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        flag.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let result = decode::frame(&path, std::path::Path::new("unused"), 0.0, &cancel);
    trigger.join().unwrap();
    assert_eq!(result.err().unwrap().to_string(), "cancelled");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
fn artifact_lifetime_and_cancelled_png_cleanup() {
    let path =
        std::env::temp_dir().join(format!("weebshot-artifact-test-{}.png", std::process::id()));
    let artifact = Arc::new(Artifact(path.clone()));
    let save_reference = artifact.clone();
    let canvas = RgbaImage::new(32, 32);
    png::save(&canvas, &path, (1, 1), &AtomicBool::new(false)).unwrap();
    assert!(!path.with_extension("pixels.png").exists());
    drop(artifact);
    assert!(path.exists());
    drop(save_reference);
    assert!(!path.exists());
    assert!(png::save(&canvas, &path, (1, 1), &AtomicBool::new(true)).is_err());
    assert!(!path.exists());
    assert!(!path.with_extension("pixels.png").exists());
}

/// Regression for the reported 4.088s 1080p pan with an animated prefix.
/// User-provided footage remains outside the repository.
#[test]
#[ignore = "requires WEEBSHOT_STITCH_SAMPLE"]
fn reported_anime_pan_pose_change() {
    let input = PathBuf::from(
        std::env::var_os("WEEBSHOT_STITCH_SAMPLE").expect("set WEEBSHOT_STITCH_SAMPLE"),
    );
    let ffmpeg = crate::export::find_tool("ffmpeg");
    for (id, range) in [(910000, Some((0.0, 4.088))), (910001, None)] {
        let job = Job {
            input: input.clone(),
            playhead: 2.0,
            duration: 4.088,
            range,
            source_fps: 24000.0 / 1001.0,
            options: Default::default(),
        };
        let stage = std::cell::RefCell::new(("", Instant::now()));
        let out = run(&ffmpeg, id, &job, &AtomicBool::new(false), |_, name| {
            let mut current = stage.borrow_mut();
            if current.0 != name {
                if !current.0.is_empty() {
                    eprintln!("{}: {:.3}s", current.0, current.1.elapsed().as_secs_f64());
                }
                *current = (name, Instant::now());
            }
        })
        .unwrap();
        eprintln!("Total: {:.3}s", out.elapsed);
        // Dense residual sampling recovers one more early frame than the
        // original 24-point grid, without crossing the pose change.
        assert_eq!(out.dimensions, (1920, 1972));
        assert_eq!(out.range, (0.625, 3.875));
        assert_eq!(out.frames, 9);
        assert!(out.confidence > 0.9);
        assert!(out.warning.contains("Omitted 2 selected frames"));
        eprintln!(
            "anime {range:?}: {:?}, frames {}, range {:?}",
            out.dimensions, out.frames, out.range
        );
        std::fs::copy(
            &out.artifact.0,
            std::env::temp_dir().join(format!("weebshot-anime-{id}.png")),
        )
        .unwrap();
    }
}

#[test]
fn native_alignment_recovers_coarse_drift() {
    let source = artwork(1400, 1000);
    let a = gray(&imageops::crop_imm(&source, 20, 10, 1280, 720).to_image());
    let b = gray(&imageops::crop_imm(&source, 20, 130, 1280, 720).to_image());
    // Both coordinates are outside the old 18px refinement radius.
    let m = motion::align(
        &motion::AlignmentFrame::new(a),
        &motion::AlignmentFrame::new(b),
        (-30, 90),
        60,
        &Options::default().native_thresholds,
    );
    assert_eq!((m.dx, m.dy), (0, 120));
    assert!(m.reliable(&Options::default().native_thresholds), "{m:?}");
}

#[test]
fn stable_component_omits_animated_prefix_and_static_tail() {
    let p = vec![
        Position::default(),
        Position::default(),
        Position::default(),
        Position { x: 0, y: -40 },
        Position { x: 0, y: -80 },
        Position::default(),
        Position::default(),
    ];
    assert_eq!(
        best_component(&p, &[0, 1, 2, 2, 2, 3, 3], (128, 96), 0.025),
        Some(2)
    );
    assert!(best_component(&[Position::default(); 3], &[0, 0, 0], (128, 96), 0.025).is_none());
    assert!(best_component(&[Position::default(); 3], &[0, 1, 2], (128, 96), 0.025).is_none());
}

#[test]
fn packed_grayscale_preserves_image_conversion() {
    let rgb = artwork(384, 320);
    assert_eq!(motion::grayscale(&rgb), imageops::grayscale(&rgb));
}

/// Compare the former full-window decode against the bounded native selection on
/// the original episode, including its seek position and network storage path.
#[test]
#[ignore = "requires WEEBSHOT_STITCH_FULL_SAMPLE"]
fn original_episode_native_decode_bound() {
    let input = PathBuf::from(
        std::env::var_os("WEEBSHOT_STITCH_FULL_SAMPLE").expect("set WEEBSHOT_STITCH_FULL_SAMPLE"),
    );
    let ffmpeg = crate::export::find_tool("ffmpeg");
    let start = 577.0;
    let end = 607.0;
    let fps = 8.0;
    let indices = [96, 112, 128];
    let before = Instant::now();
    let baseline = std::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-filter_threads",
            "1",
            "-threads",
            "2",
            "-ss",
            &format!("{start:.6}"),
            "-i",
        ])
        .arg(&input)
        .args([
            "-t",
            &format!("{:.6}", end - start),
            "-vf",
            "fps=8:start_time=0,select=eq(n\\,96)+eq(n\\,112)+eq(n\\,128)",
            "-fps_mode",
            "passthrough",
            "-threads",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    let baseline_time = before.elapsed();
    let after = Instant::now();
    let mut actual = Vec::new();
    decode::selected_frames(
        &ffmpeg,
        &input,
        start,
        end,
        fps,
        &indices,
        (1920, 1080),
        &AtomicBool::new(false),
        |_, rgb| {
            actual.extend_from_slice(rgb.as_raw());
            Ok(())
        },
    )
    .unwrap();
    let trimmed_time = after.elapsed();
    assert_eq!(
        actual, baseline.stdout,
        "Seeking changed selected source pictures"
    );
    eprintln!(
        "Original episode: native decode legacy selection {baseline_time:?}, selected window {trimmed_time:?}"
    );
}

#[test]
#[ignore = "requires WEEBSHOT_STITCH_FULL_SAMPLE"]
fn original_episode_auto_stitch() {
    let input = PathBuf::from(
        std::env::var_os("WEEBSHOT_STITCH_FULL_SAMPLE").expect("set WEEBSHOT_STITCH_FULL_SAMPLE"),
    );
    let job = Job {
        input,
        playhead: 592.0,
        duration: 1420.109,
        range: None,
        source_fps: 24000.0 / 1001.0,
        options: Default::default(),
    };
    let stage = std::cell::RefCell::new(("", Instant::now()));
    let out = run(
        &crate::export::find_tool("ffmpeg"),
        910002,
        &job,
        &AtomicBool::new(false),
        |_, name| {
            let mut current = stage.borrow_mut();
            if current.0 != name {
                if !current.0.is_empty() {
                    eprintln!("{}: {:.3}s", current.0, current.1.elapsed().as_secs_f64());
                }
                *current = (name, Instant::now());
            }
        },
    )
    .unwrap();
    assert_eq!(out.dimensions.0, 1920);
    assert!(out.dimensions.1 > 1080);
    assert_eq!(out.direction, "Up");
    assert!(out.range.0 > 588.0 && out.range.1 < 596.0);
    eprintln!(
        "Original auto stitch: {:?}, {} frames, {:?}, {:.3}s",
        out.dimensions, out.frames, out.range, out.elapsed
    );
    std::fs::copy(
        &out.artifact.0,
        std::env::temp_dir().join("weebshot-original-episode-stitch.png"),
    )
    .unwrap();
}

#[test]
fn seam_keeps_an_animated_arm_whole() {
    let base = artwork(128, 128);
    let mut pose_a = base.clone();
    let mut pose_b = base.clone();
    for y in 50..90 {
        for x in 35..55 {
            pose_a.put_pixel(x, y, Rgb([235, 190, 170]));
        }
    }
    for y in 50..65 {
        for x in 30..85 {
            pose_b.put_pixel(x, y, Rgb([235, 190, 170]));
        }
    }
    let a = imageops::crop_imm(&pose_a, 0, 32, 128, 96).to_image();
    let b = imageops::crop_imm(&pose_b, 0, 0, 128, 96).to_image();
    let mut composer = compose::Composer::new((128, 128));
    composer
        .push(&a, None, (0, 32), &AtomicBool::new(false))
        .unwrap();
    composer
        .push(&b, None, (0, 0), &AtomicBool::new(false))
        .unwrap();
    let out = imageops::colorops::grayscale(&composer.finish());
    assert!(
        out == gray(&pose_a) || out == gray(&pose_b),
        "seam spliced two different arm poses together"
    );
}

#[test]
fn missing_intermediate_frame_does_not_create_a_gap() {
    let mut composer = compose::Composer::new((128, 296));
    let frame = artwork(128, 96);
    composer
        .push(&frame, None, (0, 200), &AtomicBool::new(false))
        .unwrap();
    assert!(
        composer
            .push(&frame, None, (0, 0), &AtomicBool::new(false))
            .unwrap_err()
            .to_string()
            .contains("intermediate")
    );
}

#[test]
#[ignore = "requires WEEBSHOT_STITCH_FULL_SAMPLE"]
fn original_pan_at_different_playheads() {
    let input = PathBuf::from(
        std::env::var_os("WEEBSHOT_STITCH_FULL_SAMPLE").expect("set WEEBSHOT_STITCH_FULL_SAMPLE"),
    );
    for (n, playhead) in [591.8, 591.9, 592.0, 592.1, 592.2].into_iter().enumerate() {
        let job = Job {
            input: input.clone(),
            playhead,
            duration: 1420.109,
            range: None,
            source_fps: 24000.0 / 1001.0,
            options: Default::default(),
        };
        crate::log::clear();
        let out = run(
            &crate::export::find_tool("ffmpeg"),
            920000 + n as u64,
            &job,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(
            out.dimensions.0, 1920,
            "Changing pose introduced sideways canvas drift"
        );
        assert!(
            out.range.0 >= 590.05,
            "Unstable arm transition was retained"
        );
        crate::log::with_lines(|lines| {
            for line in lines.iter().filter(|l| l.source == "stitch").take(4) {
                eprintln!("{}", line.text);
            }
        });
        eprintln!(
            "Playhead {playhead}: {:?}, {:?}, {} frames, {:.2}s",
            out.dimensions, out.range, out.frames, out.elapsed
        );
        std::fs::copy(
            &out.artifact.0,
            std::env::temp_dir().join(format!("weebshot-pose-phase-{n}.png")),
        )
        .unwrap();
    }
}

#[test]
fn pose_change_is_not_hidden_by_flat_background() {
    let mut a = GrayImage::from_pixel(128, 96, Luma([200]));
    let mut b = a.clone();
    for y in 20..80 {
        a.put_pixel(40, y, Luma([20]));
        a.put_pixel(60, y, Luma([20]));
        b.put_pixel(70, y, Luma([20]));
        b.put_pixel(90, y, Luma([20]));
    }
    let a = motion::AlignmentFrame::new(a);
    let b = motion::AlignmentFrame::new(b);
    let m = Motion {
        dx: 0,
        dy: 0,
        error: 0.0,
        support: 1.0,
    };
    assert!(a.edge_agreement(&b, m) < Options::default().min_edge_agreement);
    assert_eq!(a.edge_agreement(&a, m), 1.0);
}

/// The reported clip combines layered motion with apparent scale changes.
#[test]
#[ignore = "requires WEEBSHOT_LAYERED_PAN_SAMPLE"]
fn reported_overlapping_layers_are_rejected() {
    let input = PathBuf::from(
        std::env::var_os("WEEBSHOT_LAYERED_PAN_SAMPLE").expect("set WEEBSHOT_LAYERED_PAN_SAMPLE"),
    );
    for (id, range, method, message) in [
        (
            910030,
            None,
            Method::Automatic,
            "two stable vertical layers",
        ),
        (
            910031,
            Some((0.0, 7.967)),
            Method::Automatic,
            "two stable vertical layers",
        ),
        (
            910039,
            Some((0.0, 7.967)),
            Method::ShiftAndScale,
            "single plane",
        ),
    ] {
        let job = Job {
            input: input.clone(),
            playhead: 4.0,
            duration: 7.967,
            range,
            source_fps: 24000.0 / 1001.0,
            options: Options {
                method,
                ..Default::default()
            },
        };
        let result = run(
            &crate::export::find_tool("ffmpeg"),
            id,
            &job,
            &AtomicBool::new(false),
            |_, _| {},
        );
        let Err(error) = result else {
            panic!("stitch accepted overlapping layers with scale changes for {range:?}");
        };
        assert!(error.to_string().contains(message), "{range:?}: {error:#}");
    }
}

#[test]
fn two_vertical_layers_round_trip() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = std::env::temp_dir().join(format!("weebshot-stitch-layers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let left = artwork(160, 430);
    let right = imageops::flip_horizontal(&artwork(160, 430));
    let count = 16;
    for n in 0..count {
        let frame = RgbImage::from_fn(320, 180, |x, y| {
            if x < 160 {
                *left.get_pixel(x, y + n * 4)
            } else {
                *right.get_pixel(x - 160, y + n * 10)
            }
        });
        frame.save(dir.join(format!("{n:03}.png"))).unwrap();
    }
    let video = dir.join("layers.mkv");
    assert!(
        std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                "8",
                "-i"
            ])
            .arg(dir.join("%03d.png"))
            .args(["-c:v", "ffv1", "-threads", "1"])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let cancel = AtomicBool::new(false);
    for (id, range, method, layer_divider_percent) in [
        (910032, Some((0.0, 2.0)), Method::Automatic, None),
        (910033, None, Method::Automatic, None),
        (
            910034,
            Some((0.0, 2.0)),
            Method::TwoVerticalLayers,
            Some(50),
        ),
    ] {
        let output = run(
            &ffmpeg,
            id,
            &Job {
                input: video.clone(),
                playhead: 1.0,
                duration: 2.0,
                range,
                source_fps: 8.0,
                options: Options {
                    method,
                    layer_divider_percent,
                    ..Default::default()
                },
            },
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(output.method, Method::TwoVerticalLayers);
        assert_eq!(output.recipe.split, Some(160));
        assert_eq!(output.dimensions, (320, 240));
        assert!(output.warning.contains("Two vertical layers"));
        assert_eq!(
            image::open(&output.artifact.0).unwrap().to_rgba8(),
            output.preview
        );
        for (x, y, p) in output.preview.enumerate_pixels() {
            let expected = if x < 160 {
                left.get_pixel(x, y)
            } else {
                right.get_pixel(x - 160, y)
            };
            assert_eq!(&p.0[..3], &expected.0);
            assert_eq!(p[3], 255);
        }
        if method == Method::TwoVerticalLayers {
            let path = dir.join("two-vertical-layers.psd");
            save_psd(
                &ffmpeg,
                910036,
                &output.recipe,
                &output.included,
                &path,
                &cancel,
                |_, _| {},
            )
            .unwrap();
            assert_psd_matches_stitch(&path, &output.preview, output.frames * 2);
        }
        let mut included = output.included.clone();
        *included.last_mut().unwrap() = false;
        let edited = recompose(
            &ffmpeg,
            id + 10,
            output.recipe,
            included,
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert!(edited.dimensions.1 < output.dimensions.1);
    }

    for (method, layer_divider_percent) in [
        (Method::Translation, None),
        (Method::TwoVerticalLayers, Some(20)),
    ] {
        let job = Job {
            input: video.clone(),
            playhead: 1.0,
            duration: 2.0,
            range: Some((0.0, 2.0)),
            source_fps: 8.0,
            options: Options {
                method,
                layer_divider_percent,
                ..Default::default()
            },
        };
        assert!(
            run(&ffmpeg, 910035, &job, &cancel, |_, _| {}).is_err(),
            "{method:?}"
        );
    }
}

#[test]
fn legacy_stitch_settings_default_to_automatic_method() {
    let options: Options = serde_json::from_str(r#"{"sampling_fps": 12}"#).unwrap();
    assert_eq!(options.method, Method::Automatic);
    assert_eq!(options.sampling_fps, 12.0);
    assert_eq!(options.layer_divider_percent, None);
}

#[test]
fn stale_advanced_thresholds_are_refreshed() {
    // Older files stored every hidden threshold without a tuning version.
    let saved: Options = ron::from_str(
        "(method: translation, selection_travel: 0.12, min_edge_agreement: 0.7, \
         native_thresholds: (max_error: 3.0, min_support: 0.9))",
    )
    .unwrap();
    assert_eq!(saved.tuning, 0);
    let current = saved.current();
    assert_eq!(current.method, Method::Translation);
    assert_eq!(current.selection_travel, 0.12);
    assert_eq!(
        current.min_edge_agreement,
        Options::default().min_edge_agreement
    );
    assert_eq!(current.native_thresholds.max_error, 8.0);
    // Hand-tuned values saved under the current tuning are kept.
    let tuned = Options {
        min_edge_agreement: 0.9,
        ..Default::default()
    };
    let restored: Options = ron::from_str(&ron::to_string(&tuned).unwrap()).unwrap();
    assert_eq!(restored.current().min_edge_agreement, 0.9);
}

#[test]
fn shift_and_scale_video_round_trip() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = std::env::temp_dir().join(format!("weebshot-stitch-zoom-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let source = imageops::blur(&artwork(430, 400), 1.5);
    for n in 0..16 {
        let width = 320 + 4 * n;
        let height = 180 + 2 * n;
        let crop = imageops::crop_imm(&source, 45 - 2 * n, 20 + 4 * n, width, height).to_image();
        let frame = imageops::resize(&crop, 320, 180, FilterType::CatmullRom);
        frame.save(dir.join(format!("{n:03}.png"))).unwrap();
    }
    let video = dir.join("zoom.mkv");
    assert!(
        std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                "8",
                "-i"
            ])
            .arg(dir.join("%03d.png"))
            .args(["-c:v", "ffv1", "-threads", "1"])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let cancel = AtomicBool::new(false);
    let output = run(
        &ffmpeg,
        910041,
        &Job {
            input: video,
            playhead: 1.0,
            duration: 2.0,
            range: Some((0.0, 2.0)),
            source_fps: 8.0,
            options: Options {
                method: Method::ShiftAndScale,
                ..Default::default()
            },
        },
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(output.method, Method::ShiftAndScale);
    assert!(
        output.dimensions.0.abs_diff(380) <= 3,
        "{:?}",
        output.dimensions
    );
    assert!(
        output.dimensions.1.abs_diff(270) <= 3,
        "{:?}",
        output.dimensions
    );
    let last = output.recipe.frames.last().unwrap();
    assert!((last.scale - 1.18).abs() < 0.02, "{}", last.scale);
    assert!(last.position.x.abs_diff(-30) <= 2, "{:?}", last.position);
    assert!(last.position.y.abs_diff(60) <= 2, "{:?}", last.position);
    let reconstructed = image::open(&output.artifact.0).unwrap().to_rgba8();
    assert_eq!(reconstructed, output.preview);
    let mut total = 0u64;
    let mut coverage = 0u64;
    for (x, y, pixel) in reconstructed.enumerate_pixels() {
        if pixel[3] == 255 {
            let expected = source.get_pixel(x + 15, y + 20);
            total += u64::from(pixel[0].abs_diff(expected[0]));
            coverage += 1;
        }
    }
    assert!(coverage > 85_000, "{coverage} covered pixels");
    assert!(
        total as f64 / (coverage as f64) < 12.0,
        "mean error {}",
        total as f64 / coverage as f64
    );
    let psd_path = dir.join("zoom.psd");
    save_psd(
        &ffmpeg,
        910043,
        &output.recipe,
        &output.included,
        &psd_path,
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert_psd_matches_stitch(&psd_path, &reconstructed, output.frames);
    let cancelled = AtomicBool::new(true);
    let absent = dir.join("cancelled.psd");
    assert!(
        save_psd(
            &ffmpeg,
            910044,
            &output.recipe,
            &output.included,
            &absent,
            &cancelled,
            |_, _| {}
        )
        .is_err()
    );
    assert!(!absent.exists());
    let mut included = output.included.clone();
    *included.last_mut().unwrap() = false;
    let edited = recompose(&ffmpeg, 910042, output.recipe, included, &cancel, |_, _| {}).unwrap();
    assert!(edited.dimensions.1 < output.dimensions.1);
}

fn assert_psd_matches_stitch(path: &std::path::Path, expected: &RgbaImage, count: usize) {
    let psd = ag_psd::read_psd(
        &std::fs::read(path).unwrap(),
        &ag_psd::psd::ReadOptions {
            use_image_data: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let composite = psd.image_data.unwrap();
    assert_eq!((composite.width, composite.height), expected.dimensions());
    assert_eq!(composite.data, *expected.as_raw());
    let layers = psd.children.unwrap();
    assert_eq!(layers.len(), count);
    let mut rebuilt = RgbaImage::new(expected.width(), expected.height());
    let mut hidden = false;
    for layer in layers {
        let image = layer.image_data.unwrap();
        let mask = layer.additional_info.mask.unwrap().image_data.unwrap();
        assert_eq!((image.width, image.height), (mask.width, mask.height));
        let x0 = layer.left.unwrap() as i32;
        let y0 = layer.top.unwrap() as i32;
        for (i, (color, mask)) in image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .zip(mask.data.as_chunks::<4>().0)
            .enumerate()
        {
            // Rotated layers have transparent corners outside the source
            // frame. Those pixels are never owned, so their mask hides them.
            if mask[0] == 0 {
                hidden |= color[3] == 255;
                continue;
            }
            assert_eq!(mask[0], 255);
            assert_eq!(color[3], 255);
            let x = x0 + (i as u32 % image.width) as i32;
            let y = y0 + (i as u32 / image.width) as i32;
            if x >= 0 && y >= 0 && x < expected.width() as i32 && y < expected.height() as i32 {
                assert_eq!(rebuilt.get_pixel(x as u32, y as u32)[3], 0);
                rebuilt.put_pixel(x as u32, y as u32, image::Rgba(*color));
            }
        }
    }
    assert!(
        hidden,
        "expected some source pixels to remain behind editable masks"
    );
    assert_eq!(rebuilt, *expected);
}

#[test]
#[ignore = "requires WEEBSHOT_LAYERED_PAN_SAMPLE"]
fn reported_overlapping_layers_experimental_preview() {
    let input = PathBuf::from(
        std::env::var_os("WEEBSHOT_LAYERED_PAN_SAMPLE").expect("set WEEBSHOT_LAYERED_PAN_SAMPLE"),
    );
    let job = Job {
        input,
        playhead: 4.0,
        duration: 7.967,
        range: Some((0.0, 7.967)),
        source_fps: 24000.0 / 1001.0,
        options: Options {
            method: Method::LayeredMotion,
            ..Default::default()
        },
    };
    let output = run(
        &crate::export::find_tool("ffmpeg"),
        910045,
        &job,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    // Several characters at different depths over a moving cube backdrop:
    // more than two planes, so only a preview is expected, not a clean result.
    assert_eq!(output.method, Method::LayeredMotion);
    assert!(output.frames >= 4);
    assert!(output.dimensions.1 > 1500, "{:?}", output.dimensions);
    assert!(output.warning.contains("foreground"), "{}", output.warning);
    eprintln!(
        "{:?}, {} frames, {:.2}s",
        output.dimensions, output.frames, output.elapsed
    );
    if let Some(path) = std::env::var_os("WEEBSHOT_LAYERED_PREVIEW") {
        std::fs::copy(&output.artifact.0, path).unwrap();
    }
    if let Some(path) = std::env::var_os("WEEBSHOT_LAYERED_PSD") {
        save_psd(
            &crate::export::find_tool("ffmpeg"),
            910046,
            &output.recipe,
            &output.included,
            &PathBuf::from(path),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
    }
}

/// Sample `source` as a camera at `center` rolled by `angle` would see it.
fn rolled_view(source: &RgbImage, center: (f32, f32), angle: f32) -> RgbImage {
    let (sin, cos) = angle.sin_cos();
    RgbImage::from_fn(320, 180, |x, y| {
        let (px, py) = (x as f32 - 160.0, y as f32 - 90.0);
        let sx = center.0 + cos * px - sin * py;
        let sy = center.1 + sin * px + cos * py;
        let (ix, iy) = (sx.floor() as u32, sy.floor() as u32);
        let (u, v) = (sx - ix as f32, sy - iy as f32);
        let p = |dx, dy| source.get_pixel(ix + dx, iy + dy);
        Rgb(std::array::from_fn(|c| {
            let top = f32::from(p(0, 0)[c]) * (1.0 - u) + f32::from(p(1, 0)[c]) * u;
            let bottom = f32::from(p(0, 1)[c]) * (1.0 - u) + f32::from(p(1, 1)[c]) * u;
            (top * (1.0 - v) + bottom * v).round() as u8
        }))
    })
}

/// A vertical pan whose camera also rolls, as in the reported Toaru clip.
/// Translation alone cannot align it; Automatic must fall back to rotation.
#[test]
fn rolling_pan_video_round_trip() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = std::env::temp_dir().join(format!("weebshot-stitch-roll-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let source = imageops::blur(&artwork(520, 520), 1.5);
    let path = |n: u32| {
        (
            (260.0 + n as f32),
            (380.0 - 9.0 * n as f32),
            0.018 * n as f32,
        )
    };
    for n in 0..16 {
        let (x, y, angle) = path(n);
        rolled_view(&source, (x, y), angle)
            .save(dir.join(format!("{n:03}.png")))
            .unwrap();
    }
    let video = dir.join("roll.mkv");
    assert!(
        std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                "8",
                "-i"
            ])
            .arg(dir.join("%03d.png"))
            .args(["-c:v", "ffv1", "-threads", "1"])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let cancel = AtomicBool::new(false);
    let job = |method, range| Job {
        input: video.clone(),
        playhead: 1.0,
        duration: 2.0,
        range,
        source_fps: 8.0,
        options: Options {
            method,
            ..Default::default()
        },
    };
    assert!(
        run(
            &ffmpeg,
            910050,
            &job(Method::Translation, Some((0.0, 2.0))),
            &cancel,
            |_, _| {}
        )
        .is_err(),
        "a 15° roll must not pass as a translation"
    );
    for (id, range) in [(910051, Some((0.0, 2.0))), (910052, None)] {
        let output = run(
            &ffmpeg,
            id,
            &job(Method::Automatic, range),
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(output.method, Method::ShiftAndScale, "{range:?}");
        assert_eq!(output.range, (0.0, 1.875), "{range:?}");
        let frames = &output.recipe.frames;
        let last = frames.last().unwrap().pose.unwrap();
        let (x, y, angle) = path(15);
        assert!((last.angle - angle).abs() < 0.004, "{last:?}");
        assert!((last.scale - 1.0).abs() < 0.005, "{last:?}");
        assert!((last.x - (x - 260.0)).abs() < 1.5, "{last:?}");
        assert!((last.y - (y - 380.0)).abs() < 1.5, "{last:?}");
        // Canvas pixel (X, Y) is frame-0 pixel (X + min.x, Y + min.y).
        let min = (
            frames.iter().map(|f| f.position.x).min().unwrap(),
            frames.iter().map(|f| f.position.y).min().unwrap(),
        );
        let reconstructed = image::open(&output.artifact.0).unwrap().to_rgba8();
        let (mut total, mut coverage) = (0u64, 0u64);
        for (cx, cy, pixel) in reconstructed.enumerate_pixels() {
            if pixel[3] == 255 {
                let sx = (cx as i32 + min.0 + 100) as u32;
                let sy = (cy as i32 + min.1 + 290) as u32;
                total += u64::from(pixel[0].abs_diff(source.get_pixel(sx, sy)[0]));
                coverage += 1;
            }
        }
        assert!(coverage > 80_000, "{coverage} covered pixels");
        let mean = total as f64 / coverage as f64;
        assert!(mean < 10.0, "mean error {mean}");
        // Rotated frames leave uncovered corners rather than black fill.
        assert!(reconstructed.pixels().any(|p| p[3] == 0));
        if range.is_none() {
            continue;
        }
        let psd_path = dir.join("roll.psd");
        save_psd(
            &ffmpeg,
            910053,
            &output.recipe,
            &output.included,
            &psd_path,
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert_psd_matches_stitch(&psd_path, &reconstructed, output.frames);
        let mut included = output.included.clone();
        *included.last_mut().unwrap() = false;
        let edited = recompose(
            &ffmpeg,
            910054,
            output.recipe.clone(),
            included,
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert!(edited.dimensions.1 < output.dimensions.1);
    }
}

/// User-provided footage remains outside the repository: an upward pan whose
/// camera rolls about 29° while it decelerates.
#[test]
#[ignore = "requires WEEBSHOT_ROLLING_PAN_SAMPLE"]
fn reported_rolling_pan() {
    let input = PathBuf::from(
        std::env::var_os("WEEBSHOT_ROLLING_PAN_SAMPLE").expect("set WEEBSHOT_ROLLING_PAN_SAMPLE"),
    );
    let ffmpeg = crate::export::find_tool("ffmpeg");
    // The playhead rests at 0 right after the clip is opened, on a pair the
    // translation matcher rejects because the roll has already begun.
    for (id, method, range, playhead) in [
        (910060, Method::Automatic, Some((0.0, 3.379)), 1.0),
        (910061, Method::Automatic, None, 1.0),
        (910062, Method::ShiftAndScale, None, 1.0),
        (910063, Method::Automatic, None, 0.0),
        (910064, Method::Automatic, None, 3.379),
    ] {
        let job = Job {
            input: input.clone(),
            playhead,
            duration: 3.379,
            range,
            source_fps: 24000.0 / 1001.0,
            options: Options {
                method,
                ..Default::default()
            },
        };
        let stage = std::cell::RefCell::new(("", Instant::now()));
        let out = run(&ffmpeg, id, &job, &AtomicBool::new(false), |_, name| {
            let mut current = stage.borrow_mut();
            if current.0 != name {
                if !current.0.is_empty() {
                    eprintln!("  {}: {:.3}s", current.0, current.1.elapsed().as_secs_f64());
                }
                *current = (name, Instant::now());
            }
        })
        .unwrap();
        eprintln!(
            "rolling {method:?} {range:?}: {:?}, {} frames, {:?}, {:.1}s",
            out.dimensions, out.frames, out.range, out.elapsed
        );
        assert_eq!(out.method, Method::ShiftAndScale);
        assert!(out.range.0 < 0.05 && out.range.1 > 3.2, "{:?}", out.range);
        assert!(
            out.dimensions.0 > 2200 && out.dimensions.1 > 2000,
            "{:?}",
            out.dimensions
        );
        let roll = out.recipe.frames.last().unwrap().pose.unwrap().angle;
        assert!((0.45..0.58).contains(&roll), "roll {roll}");
        if let Some(path) = std::env::var_os("WEEBSHOT_ROLLING_PREVIEW") {
            std::fs::copy(
                &out.artifact.0,
                PathBuf::from(path).join(format!("{id}.png")),
            )
            .unwrap();
        }
    }
}

/// How well consecutive frames of a translation stitch agree where they
/// overlap: the share of overlapping pixels (every other pixel) whose
/// grayscale values differ by more than 24. Animation inside the overlap
/// counts too, so this ranks results rather than proving an error. `None`
/// for methods that warp or split frames.
fn seam_mismatch(ffmpeg: &std::path::Path, out: &Output) -> Option<f32> {
    let recipe = &out.recipe;
    if recipe.method != Method::Translation || recipe.split.is_some() {
        return None;
    }
    let frames: Vec<_> = recipe
        .frames
        .iter()
        .zip(&out.included)
        .filter(|(_, included)| **included)
        .map(|(frame, _)| frame)
        .collect();
    let indices: Vec<_> = frames.iter().map(|f| f.sample).collect();
    let mut previous: Option<GrayImage> = None;
    let mut shares = Vec::new();
    decode::selected_frames(
        ffmpeg,
        &recipe.input,
        recipe.start,
        recipe.end,
        recipe.fps,
        &indices,
        recipe.native,
        &AtomicBool::new(false),
        |n, rgb| {
            let gray = motion::grayscale(&rgb);
            if let Some(older) = &previous {
                let (dx, dy) = frames[n].position.delta(frames[n - 1].position);
                let (w, h) = (gray.width() as i32, gray.height() as i32);
                let (mut total, mut bad) = (0u32, 0u32);
                for y in (0.max(-dy)..h.min(h - dy)).step_by(2) {
                    for x in (0.max(-dx)..w.min(w - dx)).step_by(2) {
                        total += 1;
                        let a = older.get_pixel((x + dx) as u32, (y + dy) as u32)[0];
                        bad += u32::from(a.abs_diff(gray.get_pixel(x as u32, y as u32)[0]) > 24);
                    }
                }
                if total > 0 {
                    shares.push(bad as f32 / total as f32);
                }
            }
            previous = Some(gray);
            Ok(())
        },
    )
    .ok()?;
    (!shares.is_empty()).then(|| shares.iter().sum::<f32>() / shares.len() as f32)
}

/// Benchmark over local footage. `WEEBSHOT_BENCH` names a list with one
/// `path` or `path|in|out` per line. Every clip runs three ways: `range`
/// (the whole window marked In/Out), `start` (Automatic with the playhead
/// where it rests after opening the clip) and `mid` (Automatic from the
/// window's midpoint). `WEEBSHOT_BENCH_OUT` receives `<line>-<mode>.png`
/// previews and `results.tsv`.
///
/// `WEEBSHOT_BENCH_BASELINE` names an earlier `results.tsv`; differences are
/// printed as `DIFF` lines and, with `WEEBSHOT_BENCH_STRICT`, a result that
/// was lost or whose seams got worse fails the test. Other switches:
/// `WEEBSHOT_BENCH_ONLY=3,7` (line numbers), `WEEBSHOT_BENCH_MODES=start,mid`,
/// `WEEBSHOT_BENCH_METHOD=layered|scale|translation`, `WEEBSHOT_BENCH_FORCE`,
/// `WEEBSHOT_BENCH_JOBS` (parallel stitches, default half the cores).
#[test]
#[ignore = "requires WEEBSHOT_BENCH"]
fn local_footage_benchmark() {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex;
    let list = std::fs::read_to_string(std::env::var_os("WEEBSHOT_BENCH").unwrap()).unwrap();
    let out_dir = PathBuf::from(std::env::var_os("WEEBSHOT_BENCH_OUT").unwrap());
    std::fs::create_dir_all(&out_dir).unwrap();
    let ffmpeg = crate::export::find_tool("ffmpeg");
    let listed = |name: &str, value: &str| {
        std::env::var(name).map_or(true, |v| v.split(',').any(|k| k == value))
    };
    struct Clip {
        n: usize,
        key: String,
        input: PathBuf,
        window: (f64, f64),
        fps: f64,
        duration: f64,
    }
    let mut clips = Vec::new();
    for (n, line) in list.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        if !listed("WEEBSHOT_BENCH_ONLY", &n.to_string()) {
            continue;
        }
        let mut parts = line.trim().split('|');
        let input = PathBuf::from(parts.next().unwrap());
        let window: Option<(f64, f64)> = parts
            .next()
            .map(|a| (a.parse().unwrap(), parts.next().unwrap().parse().unwrap()));
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=r_frame_rate:format=duration",
                "-of",
                "csv=p=0",
            ])
            .arg(&input)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&probe.stdout);
        let mut lines = text.lines();
        let rate = lines.next().unwrap_or("24/1").trim_end_matches(',');
        let (a, b) = rate.split_once('/').unwrap_or((rate, "1"));
        let fps = a.parse::<f64>().unwrap() / b.parse::<f64>().unwrap();
        let duration: f64 = lines.next().unwrap().trim().parse().unwrap();
        let file = input.file_name().unwrap().to_string_lossy().into_owned();
        clips.push(Clip {
            n,
            key: match window {
                Some((a, b)) => format!("{file}@{a:.1}-{b:.1}"),
                None => file,
            },
            input,
            window: window.unwrap_or((0.0, duration)),
            fps,
            duration,
        });
    }
    const MODES: [&str; 3] = ["range", "start", "mid"];
    let queue: Mutex<VecDeque<(usize, &str)>> = Mutex::new(
        (0..clips.len())
            .flat_map(|c| MODES.map(|mode| (c, mode)))
            .filter(|(_, mode)| listed("WEEBSHOT_BENCH_MODES", mode))
            .collect(),
    );
    // Columns of results.tsv after the clip and mode.
    struct Row {
        ok: bool,
        method: String,
        size: (u32, u32),
        frames: usize,
        range: (f64, f64),
        omitted: usize,
        forced: bool,
        mismatch: Option<f32>,
        seconds: f32,
        message: String,
    }
    let rows: Mutex<BTreeMap<(usize, usize), Row>> = Mutex::new(BTreeMap::new());
    let jobs = std::env::var("WEEBSHOT_BENCH_JOBS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() / 2))
        .max(1);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let Some((c, mode)) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    let clip = &clips[c];
                    let (lo, hi) = clip.window;
                    let m = MODES.iter().position(|k| *k == mode).unwrap();
                    let job = Job {
                        input: clip.input.clone(),
                        playhead: if mode == "start" { lo } else { (lo + hi) / 2.0 },
                        duration: clip.duration,
                        range: (mode == "range").then_some((lo, hi)),
                        source_fps: clip.fps,
                        options: Options {
                            method: match std::env::var("WEEBSHOT_BENCH_METHOD").as_deref() {
                                Ok("layered") => Method::LayeredMotion,
                                Ok("scale") => Method::ShiftAndScale,
                                Ok("translation") => Method::Translation,
                                _ => Method::Automatic,
                            },
                            force: std::env::var_os("WEEBSHOT_BENCH_FORCE").is_some(),
                            ..Default::default()
                        },
                    };
                    let started = Instant::now();
                    let id = 990000 + clip.n as u64 * 10 + m as u64;
                    let result = run(&ffmpeg, id, &job, &AtomicBool::new(false), |_, _| {});
                    let seconds = started.elapsed().as_secs_f32();
                    let row = match result {
                        Ok(out) => {
                            out.preview
                                .save(out_dir.join(format!("{}-{mode}.png", clip.n)))
                                .unwrap();
                            Row {
                                ok: true,
                                method: format!("{:?}", out.method),
                                size: out.dimensions,
                                frames: out.frames,
                                range: out.range,
                                omitted: out.omitted_frames,
                                forced: out.forced,
                                mismatch: seam_mismatch(&ffmpeg, &out),
                                seconds,
                                message: String::new(),
                            }
                        }
                        Err(error) => Row {
                            ok: false,
                            method: String::new(),
                            size: (0, 0),
                            frames: 0,
                            range: (0.0, 0.0),
                            omitted: 0,
                            forced: false,
                            mismatch: None,
                            seconds,
                            message: format!("{error:#}").replace(['\t', '\n'], " "),
                        },
                    };
                    rows.lock().unwrap().insert((c, m), row);
                }
            });
        }
    });
    let rows = rows.into_inner().unwrap();
    let mut tsv = String::from(
        "clip\tmode\tstatus\tmethod\twidth\theight\tframes\tstart\tend\tomitted\tforced\tmismatch\tseconds\tmessage\n",
    );
    for (&(c, m), row) in &rows {
        let mismatch = row.mismatch.map_or(String::new(), |v| format!("{v:.3}"));
        tsv += &format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{}\t{}\t{mismatch}\t{:.1}\t{}\n",
            clips[c].key,
            MODES[m],
            if row.ok { "OK" } else { "ERR" },
            row.method,
            row.size.0,
            row.size.1,
            row.frames,
            row.range.0,
            row.range.1,
            row.omitted,
            row.forced,
            row.seconds,
            row.message
        );
        eprintln!(
            "BENCH {:>3} {:<5} {} {}",
            clips[c].n,
            MODES[m],
            if row.ok {
                format!(
                    "OK{} {} {}x{} frames {} range {:.2}-{:.2} omitted {} mismatch {}",
                    if row.forced { " FORCED" } else { "" },
                    row.method,
                    row.size.0,
                    row.size.1,
                    row.frames,
                    row.range.0,
                    row.range.1,
                    row.omitted,
                    if mismatch.is_empty() { "-" } else { &mismatch }
                )
            } else {
                format!("ERR {}", row.message)
            },
            clips[c].key
        );
    }
    std::fs::write(out_dir.join("results.tsv"), &tsv).unwrap();
    // Summary: what a user sees depends on the playhead only where start and
    // mid disagree; two-frame results are stitches in name only.
    for (m, mode) in MODES.iter().enumerate() {
        let of_mode: Vec<_> = rows
            .iter()
            .filter(|(k, _)| k.1 == m)
            .map(|(_, r)| r)
            .collect();
        if of_mode.is_empty() {
            continue;
        }
        let ok: Vec<_> = of_mode.iter().filter(|r| r.ok).collect();
        let mut mismatches: Vec<f32> = ok.iter().filter_map(|r| r.mismatch).collect();
        mismatches.sort_by(f32::total_cmp);
        eprintln!(
            "SUMMARY {mode:<5} {}/{} OK, {} forced, {} two-frame, median seam mismatch {:.3}, worst {:.3}",
            ok.len(),
            of_mode.len(),
            ok.iter().filter(|r| r.forced).count(),
            ok.iter().filter(|r| r.frames == 2).count(),
            mismatches.get(mismatches.len() / 2).copied().unwrap_or(0.0),
            mismatches.last().copied().unwrap_or(0.0)
        );
    }
    let disagree = (0..clips.len())
        .filter(|&c| {
            rows.get(&(c, 1))
                .zip(rows.get(&(c, 2)))
                .is_some_and(|(a, b)| a.ok != b.ok)
        })
        .count();
    eprintln!("SUMMARY start and mid disagree on {disagree} clips");
    let Some(baseline) = std::env::var_os("WEEBSHOT_BENCH_BASELINE") else {
        return;
    };
    let baseline = std::fs::read_to_string(baseline).unwrap();
    let mut failures = 0;
    let mut differences = 0;
    for line in baseline.lines().skip(1) {
        let f: Vec<_> = line.split('\t').collect();
        let Some(c) = clips.iter().position(|clip| clip.key == f[0]) else {
            continue;
        };
        let Some(row) = MODES
            .iter()
            .position(|m| *m == f[1])
            .and_then(|m| rows.get(&(c, m)))
        else {
            continue;
        };
        let was_ok = f[2] == "OK";
        let area = |w: f64, h: f64| w * h;
        let before = area(f[4].parse().unwrap(), f[5].parse().unwrap());
        let after = area(row.size.0.into(), row.size.1.into());
        let seam = f[11].parse::<f32>().ok().zip(row.mismatch);
        let change = match (was_ok, row.ok) {
            (true, false) => Some(("LOST", true)),
            (false, true) => Some(("GAINED", false)),
            (false, false) => None,
            (true, true) if seam.is_some_and(|(a, b)| b > a + 0.02) => Some(("SEAMS WORSE", true)),
            (true, true) if (after - before).abs() > before * 0.05 => Some(("SIZE", false)),
            (true, true) if seam.is_some_and(|(a, b)| b < a - 0.02) => {
                Some(("SEAMS BETTER", false))
            }
            (true, true) => None,
        };
        if let Some((kind, failure)) = change {
            differences += 1;
            failures += usize::from(failure);
            eprintln!(
                "DIFF {kind:<12} {:>3} {:<5} {}x{} mismatch {} -> {} {}",
                clips[c].n,
                f[1],
                f[4],
                f[5],
                f[11],
                if row.ok {
                    format!(
                        "{}x{} mismatch {}",
                        row.size.0,
                        row.size.1,
                        row.mismatch.map_or("-".into(), |v| format!("{v:.3}"))
                    )
                } else {
                    format!("ERR {}", row.message)
                },
                clips[c].key
            );
        }
    }
    eprintln!(
        "SUMMARY {differences} differences from the baseline, {failures} of them regressions"
    );
    assert!(
        failures == 0 || std::env::var_os("WEEBSHOT_BENCH_STRICT").is_none(),
        "{failures} regressions from the baseline"
    );
}

/// A character cel that pans slower than its background. The layered method
/// must keep the sprite whole and rebuild the background around it.
#[test]
fn layered_planes_video_round_trip() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = std::env::temp_dir().join(format!("weebshot-stitch-planes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let background = imageops::blur(&artwork(640, 520), 1.0);
    // An outlined ellipse with its own texture, distinct from the background.
    let texture = artwork(280, 200);
    let inside = |x: u32, y: u32| {
        let (dx, dy) = ((x as f32 - 140.0) / 136.0, (y as f32 - 100.0) / 96.0);
        dx * dx + dy * dy
    };
    let sprite = RgbImage::from_fn(280, 200, |x, y| {
        let r = inside(x, y);
        if r > 0.88 {
            Rgb([20, 15, 30])
        } else {
            let p = texture.get_pixel(279 - x, y);
            Rgb([255 - p[0], 200 - p[1] / 2, p[2] / 2 + 60])
        }
    });
    let (sprite_x, sprite_y) = (180u32, 140u32);
    let frames = 16u32;
    for n in 0..frames {
        let mut frame = imageops::crop_imm(&background, 0, 8 * n, 640, 360).to_image();
        for (x, y, p) in sprite.enumerate_pixels() {
            if inside(x, y) <= 1.0 {
                frame.put_pixel(sprite_x + x, sprite_y - 3 * n + y, *p);
            }
        }
        frame.save(dir.join(format!("{n:03}.png"))).unwrap();
    }
    let video = dir.join("planes.mkv");
    assert!(
        std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                "8",
                "-i"
            ])
            .arg(dir.join("%03d.png"))
            .args(["-c:v", "ffv1", "-threads", "1"])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let cancel = AtomicBool::new(false);
    let job = |method| Job {
        input: video.clone(),
        playhead: 1.0,
        duration: 2.0,
        range: Some((0.0, 2.0)),
        source_fps: 8.0,
        options: Options {
            method,
            ..Default::default()
        },
    };
    let output = run(
        &ffmpeg,
        910070,
        &job(Method::LayeredMotion),
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(output.method, Method::LayeredMotion);
    let canvas = image::open(&output.artifact.0).unwrap().to_rgba8();
    assert_eq!(canvas.width(), 640);
    assert!(
        canvas.height().abs_diff(360 + 8 * 15) <= 3,
        "{:?}",
        canvas.dimensions()
    );
    // The foreground shows the sprite as it was at the middle selected frame.
    let frames = &output.recipe.frames;
    let middle = frames[frames.len() / 2].sample as u32;
    let top = frames.iter().map(|f| f.position.y).min().unwrap();
    let (fx, fy) = (sprite_x, (sprite_y + 5 * middle) as i32 - top);
    let mut sprite_error = (0u64, 0u64);
    let mut background_error = (0u64, 0u64);
    for (x, y, pixel) in canvas.enumerate_pixels() {
        let (sx, sy) = (x as i32 - fx as i32, y as i32 - fy);
        let on_sprite = sx >= 0 && sy >= 0 && sx < 280 && sy < 200;
        let r = if on_sprite {
            inside(sx as u32, sy as u32)
        } else {
            9.0
        };
        if r < 0.8 {
            let expected = sprite.get_pixel(sx as u32, sy as u32);
            sprite_error.0 += u64::from(pixel[0].abs_diff(expected[0]));
            sprite_error.1 += 1;
        } else if r > 1.6 && pixel[3] == 255 {
            let by = (y as i32 + top) as u32;
            if by < 520 {
                let expected = background.get_pixel(x, by);
                background_error.0 += u64::from(pixel[0].abs_diff(expected[0]));
                background_error.1 += 1;
            }
        }
    }
    let mean = |(total, count): (u64, u64)| total as f64 / count.max(1) as f64;
    assert!(sprite_error.1 > 30_000, "sprite pixels {}", sprite_error.1);
    assert!(
        mean(sprite_error) < 6.0,
        "sprite error {}",
        mean(sprite_error)
    );
    assert!(
        background_error.1 > 150_000,
        "background {}",
        background_error.1
    );
    assert!(
        mean(background_error) < 6.0,
        "background error {}",
        mean(background_error)
    );
    // Layers: one background and one foreground layer per frame, foreground
    // on top, and their masked union reproduces the merged image.
    let psd_path = dir.join("planes.psd");
    save_psd(
        &ffmpeg,
        910071,
        &output.recipe,
        &output.included,
        &psd_path,
        &cancel,
        |_, _| {},
    )
    .unwrap();
    let psd = ag_psd::read_psd(
        &std::fs::read(&psd_path).unwrap(),
        &ag_psd::psd::ReadOptions {
            use_image_data: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(psd.image_data.unwrap().data, *canvas.as_raw());
    let layers = psd.children.unwrap();
    assert_eq!(layers.len(), 2 * output.frames);
    let mut rebuilt = RgbaImage::new(canvas.width(), canvas.height());
    for layer in layers {
        let image = layer.image_data.unwrap();
        let mask = layer.additional_info.mask.unwrap().image_data.unwrap();
        let (x0, y0) = (layer.left.unwrap() as i32, layer.top.unwrap() as i32);
        for (i, (color, mask)) in image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .zip(mask.data.as_chunks::<4>().0)
            .enumerate()
        {
            let (x, y) = (
                x0 + (i as u32 % image.width) as i32,
                y0 + (i as u32 / image.width) as i32,
            );
            if mask[0] == 255
                && color[3] == 255
                && x >= 0
                && y >= 0
                && (x as u32) < canvas.width()
                && (y as u32) < canvas.height()
            {
                rebuilt.put_pixel(x as u32, y as u32, image::Rgba(*color));
            }
        }
    }
    let differing = rebuilt
        .pixels()
        .zip(canvas.pixels())
        .filter(|(a, b)| a != b)
        .count();
    // Background holes filled from masked frames have no owning layer.
    assert!(
        differing * 200 < (canvas.width() * canvas.height()) as usize,
        "{differing} pixels differ between the layers and the merged image"
    );
    // Frame edits reuse the masks.
    let mut included = output.included.clone();
    *included.last_mut().unwrap() = false;
    let count = frames.len();
    let edited = recompose(
        &ffmpeg,
        910072,
        output.recipe.clone(),
        included,
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert!(edited.dimensions.1 < canvas.height());
    assert!(count >= 3);
    // A single translating plane is not layered.
    let flat = dir.join("flat.mkv");
    for n in 0..16u32 {
        imageops::crop_imm(&background, 0, 8 * n, 640, 360)
            .to_image()
            .save(dir.join(format!("{n:03}.png")))
            .unwrap();
    }
    assert!(
        std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                "8",
                "-i"
            ])
            .arg(dir.join("%03d.png"))
            .args(["-c:v", "ffv1", "-threads", "1"])
            .arg(&flat)
            .status()
            .unwrap()
            .success()
    );
    let single = Job {
        input: flat,
        ..job(Method::LayeredMotion)
    };
    let error = run(&ffmpeg, 910073, &single, &cancel, |_, _| {})
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("two independently moving layers"),
        "{error}"
    );
}

/// Every frame carries a different large overlay, so no pair passes the
/// alignment checks. The stitch is refused unless forced; a forced result is
/// marked as such and still follows the pan, and it never joins the next shot.
#[test]
fn forced_result_when_alignment_is_refused() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("FFmpeg unavailable; video fixture test skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("weebshot-forced-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let source = artwork(384, 640);
    let (w, h) = crop(&source, 0, 0).dimensions();
    for i in 0..24u32 {
        let mut frame = crop(&source, 20, 10 + i * 6);
        // A "character" filling the middle of the frame that changes every frame.
        for y in h / 5..h * 4 / 5 {
            for x in w / 5..w * 4 / 5 {
                let v = ((x * (7 + i) + y * (13 + 3 * i) + i * 97) % 256) as u8;
                frame.put_pixel(x, y, Rgb([v, 255 - v, (v / 2).wrapping_add(i as u8 * 9)]));
            }
        }
        frame.save(dir.join(format!("{i:03}.png"))).unwrap();
    }
    // A second, unrelated shot after the pan.
    for i in 24..32u32 {
        let mut frame = crop(&source, 20, 10);
        for (x, y, p) in frame.enumerate_pixels_mut() {
            *p = Rgb([((x * 131 + y * 73 + i) % 256) as u8, 0, 255]);
        }
        frame.save(dir.join(format!("{i:03}.png"))).unwrap();
    }
    let video = dir.join("pan.mkv");
    let status = std::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-framerate",
            "8",
            "-i",
        ])
        .arg(dir.join("%03d.png"))
        .args(["-c:v", "ffv1"])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());
    let job = |force| Job {
        input: video.clone(),
        playhead: 0.0,
        duration: 4.0,
        range: None,
        source_fps: 8.0,
        options: Options {
            force,
            ..Default::default()
        },
    };
    let cancel = AtomicBool::new(false);
    assert!(run(&ffmpeg, 910070, &job(false), &cancel, |_, _| {}).is_err());
    let out = run(&ffmpeg, 910071, &job(true), &cancel, |_, _| {}).unwrap();
    assert!(out.forced);
    assert!(out.warning.starts_with("Forced result"));
    assert_eq!(out.method, Method::Translation);
    assert_eq!(out.dimensions.0, w);
    // 23 steps of 6 px were panned; most of that must be recovered.
    assert!(
        out.dimensions.1 > h + 100 && out.dimensions.1 <= h + 138,
        "{:?}",
        out.dimensions
    );
    assert!(out.range.1 < 3.0, "joined the next shot: {:?}", out.range);
}

#[test]
fn other_regions_are_ordered_by_distance_and_need_travel() {
    // A one-pixel twitch, a real pan, a cut, then a second pan.
    let mut motions = vec![moving(0, 1), moving(0, 0), None];
    motions.extend(std::iter::repeat_n(moving(0, 6), 4));
    motions.push(None);
    motions.extend(std::iter::repeat_n(moving(8, 0), 4));
    let options = Options::default();
    assert_eq!(
        other_regions(&motions, Some(0), &options, 8.0, (320, 180)),
        vec![(3, 7), (8, 12)]
    );
    assert_eq!(
        other_regions(&motions, Some(12), &options, 8.0, (320, 180)),
        vec![(8, 12), (3, 7)]
    );
    // Without a playhead (inside a marked range) the pan that travels the
    // larger share of the frame comes first: 24 of 180 rows beats 32 of 320 columns.
    assert_eq!(
        other_regions(&motions, None, &options, 8.0, (320, 180)),
        vec![(3, 7), (8, 12)]
    );
}

/// A small camera movement under the playhead must not hide the real pan
/// after it; when there is nothing larger, the small result is still
/// returned and says so.
#[test]
fn small_result_gives_way_to_a_larger_pan() {
    let ffmpeg = crate::export::find_tool("ffmpeg");
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("FFmpeg unavailable; video fixture test skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("weebshot-small-test-{}", std::process::id()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let source = artwork(384, 320);
    // Three 2 px steps (6% of the frame height), then a hold.
    let mut twitch: Vec<u32> = (0..4).map(|i| 10 + i * 2).collect();
    twitch.extend([16; 10]);
    let mut with_pan = twitch.clone();
    with_pan.extend((1..=16).map(|i| 16 + i * 4));
    let run_on = |name: &str, rows: &[u32], id: u64| {
        let frames = dir.join(name);
        std::fs::create_dir_all(&frames).unwrap();
        for (i, y) in rows.iter().enumerate() {
            crop(&source, 20, *y)
                .save(frames.join(format!("{i:03}.png")))
                .unwrap();
        }
        let video = dir.join(format!("{name}.mkv"));
        let status = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                "8",
                "-i",
            ])
            .arg(frames.join("%03d.png"))
            .args(["-c:v", "ffv1"])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());
        let job = Job {
            input: video,
            playhead: 0.0,
            duration: rows.len() as f64 / 8.0,
            range: None,
            source_fps: 8.0,
            options: Options::default(),
        };
        run(&ffmpeg, id, &job, &AtomicBool::new(false), |_, _| {}).unwrap()
    };
    let alone = run_on("twitch", &twitch, 910080);
    assert_eq!(alone.dimensions, (128, 102));
    assert!(alone.gain < SMALL_GAIN);
    assert!(
        alone.warning.starts_with("Small result"),
        "{}",
        alone.warning
    );
    assert!(alone.artifact.0.exists());
    let larger = run_on("pan", &with_pan, 910081);
    assert_eq!(larger.dimensions, (128, 96 + 64));
    assert!(larger.gain > 0.6);
    // The pan's region may start on the hold before it, but after the twitch.
    assert!(larger.range.0 > 0.5, "{:?}", larger.range);
    assert!(larger.artifact.0.exists());
}

/// Lighting fixed to the screen makes each frame brighter towards its bottom
/// edge, so consecutive frames disagree by a constant amount where they
/// overlap. Evening must remove the step at the seams without touching frames
/// that already agree.
#[test]
fn screen_fixed_lighting_leaves_no_seam_step() {
    let source = artwork(160, 400);
    let frame = |y: u32, lit: bool| {
        let mut frame = crop(&source, 16, y);
        if lit {
            for (_, row, pixel) in frame.enumerate_pixels_mut() {
                let glow = (row * 30 / 95) as u8;
                *pixel = Rgb(pixel.0.map(|v| (v / 2 + 40).saturating_add(glow)));
            }
        }
        frame
    };
    let cancel = AtomicBool::new(false);
    // Largest jump between neighbouring rows of the lighting that remains
    // once the artwork is subtracted.
    let step = |lit: bool, even: bool| {
        let mut composer = compose::Composer::new((128, 96 + 4 * 32));
        let mut changed = 0;
        for i in 0..5 {
            let offset = (0, i * 32);
            let original = frame(10 + i * 32, lit);
            let evened = if even {
                composer.matched(&original, None, offset, &cancel).unwrap()
            } else {
                None
            };
            changed += usize::from(evened.is_some());
            composer
                .push(evened.as_ref().unwrap_or(&original), None, offset, &cancel)
                .unwrap();
        }
        let canvas = composer.finish();
        let rows: Vec<f32> = (0..canvas.height())
            .map(|y| {
                (0..128)
                    .map(|x| {
                        let art = source.get_pixel(16 + x, 10 + y)[0];
                        let base = if lit { art / 2 + 40 } else { art };
                        f32::from(canvas.get_pixel(x, y)[0]) - f32::from(base)
                    })
                    .sum::<f32>()
                    / 128.0
            })
            .collect();
        let worst = rows
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max);
        (worst, changed)
    };
    let (hard, _) = step(true, false);
    let (evened, changed) = step(true, true);
    assert!(hard > 6.0, "fixture has no seam step: {hard}");
    assert!(evened < 2.0, "seam step remains: {evened}");
    assert_eq!(changed, 4);
    assert_eq!(step(false, true), (0.0, 0));
}

/// A coloured light that is in one frame and gone from the next differs by
/// far more than screen-fixed lighting. The seam must still close, but the
/// later frame keeps its own light away from the seam.
#[test]
fn fading_light_closes_the_seam_without_repainting_the_frame() {
    let source = artwork(160, 400);
    let base = |x: u32, y: u32| source.get_pixel(16 + x, 10 + y).0.map(|v| v / 2 + 40);
    let beam = |x: u32| {
        let t = (x as f32 / 127.0 * std::f32::consts::PI).sin();
        (60.0 * t * t) as u8
    };
    let frame = |y: u32, lit: bool| {
        RgbImage::from_fn(128, 96, |x, row| {
            let mut pixel = base(x, y + row);
            if lit {
                pixel[1] += beam(x);
            }
            Rgb(pixel)
        })
    };
    let cancel = AtomicBool::new(false);
    let compose = |even: bool| {
        let mut composer = compose::Composer::new((128, 96 + 32));
        composer
            .push(&frame(0, true), None, (0, 0), &cancel)
            .unwrap();
        let second = frame(32, false);
        let evened = if even {
            composer.matched(&second, None, (0, 32), &cancel).unwrap()
        } else {
            None
        };
        assert_eq!(evened.is_some(), even);
        composer
            .push(evened.as_ref().unwrap_or(&second), None, (0, 32), &cancel)
            .unwrap();
        composer.finish()
    };
    // Green light left in each row under the middle of the beam.
    let light = |canvas: &RgbaImage| -> Vec<f32> {
        (0..canvas.height())
            .map(|y| {
                (48..80)
                    .map(|x| f32::from(canvas.get_pixel(x, y)[1]) - f32::from(base(x, y)[1]))
                    .sum::<f32>()
                    / 32.0
            })
            .collect()
    };
    let step = |rows: &[f32]| {
        rows.windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    };
    let hard = light(&compose(false));
    let evened = light(&compose(true));
    assert!(
        step(&hard) > 30.0,
        "fixture has no seam step: {}",
        step(&hard)
    );
    assert!(step(&evened) < 8.0, "seam step remains: {}", step(&evened));
    assert!(evened[0] > 50.0, "first frame changed: {}", evened[0]);
    let last = evened[evened.len() - 1];
    assert!(last < 10.0, "light carried across the frame: {last}");
}
