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
            .push(&artwork(128, 96), (0, 0), &AtomicBool::new(true))
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

    // An explicit cut must fail; automatic discovery must stop before it.
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
    assert!(
        run(
            &ffmpeg,
            900004,
            &cut_job,
            &AtomicBool::new(false),
            |_, _| {}
        )
        .is_err()
    );
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
        assert_eq!(out.dimensions, (1920, 1924));
        assert_eq!(out.range, (0.75, 4.0));
        assert_eq!(out.frames, 10);
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
    composer.push(&a, (0, 32), &AtomicBool::new(false)).unwrap();
    composer.push(&b, (0, 0), &AtomicBool::new(false)).unwrap();
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
        .push(&frame, (0, 200), &AtomicBool::new(false))
        .unwrap();
    assert!(
        composer
            .push(&frame, (0, 0), &AtomicBool::new(false))
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
            assert_eq!(color[3], 255);
            if mask[0] == 0 {
                hidden = true;
                continue;
            }
            assert_eq!(mask[0], 255);
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
    assert_eq!(output.method, Method::LayeredMotion);
    assert!(output.frames >= 4);
    assert!(output.dimensions.1 > 1900, "{:?}", output.dimensions);
    assert!(output.warning.contains("visible seams"));
    eprintln!(
        "{:?}, {} frames, {:.2}s",
        output.dimensions, output.frames, output.elapsed
    );
    if let Some(path) = std::env::var_os("WEEBSHOT_LAYERED_PREVIEW") {
        std::fs::copy(&output.artifact.0, path).unwrap();
    }
    if let Some(path) = std::env::var_os("WEEBSHOT_LAYERED_PSD") {
        let path = PathBuf::from(path);
        save_psd(
            &crate::export::find_tool("ffmpeg"),
            910046,
            &output.recipe,
            &output.included,
            &path,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        let png = image::open(&output.artifact.0).unwrap().to_rgba8();
        assert_psd_matches_stitch(&path, &png, output.frames);
    }
    let mut background = job;
    background.options.prefer_background = true;
    let output_background = run(
        &crate::export::find_tool("ffmpeg"),
        910047,
        &background,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert!(output_background.dimensions.1 < output.dimensions.1);
}
