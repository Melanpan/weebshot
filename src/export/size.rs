//! Bitrate math for "fit the clip into N MB" exports.

use anyhow::{Result, bail};

/// Fraction of the size budget we actually aim for, leaving room for container
/// overhead and encoder overshoot.
const SAFETY: f64 = 0.95;

/// Lowest video bitrate we're willing to produce before telling the user to
/// shorten the clip instead.
const MIN_VIDEO_KBPS: f64 = 100.0;

/// Video bitrate (kbit/s) so that video + audio for `duration` seconds fit in `target_mb` MiB.
pub fn video_kbps(target_mb: f64, duration: f64, audio_kbps: u32) -> Result<u32> {
    if duration <= 0.0 {
        bail!("clip has no duration");
    }
    let total_kbps = target_mb * 1024.0 * 1024.0 * 8.0 / 1000.0 * SAFETY / duration;
    let video = total_kbps - audio_kbps as f64;
    if video < MIN_VIDEO_KBPS {
        bail!(
            "{target_mb} MB is too small for a {duration:.1}s clip (≈{video:.0} kbps video). \
             Shorten the clip, drop the audio or raise the size limit."
        );
    }
    Ok(video as u32)
}

/// Suggest a lower resolution when the bitrate is too low to look good at 1080p.
pub fn suggested_height(video_kbps: u32) -> Option<u32> {
    match video_kbps {
        0..400 => Some(360),
        400..900 => Some(480),
        900..2000 => Some(720),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_budget() {
        // 4 MiB over 30s with 96k audio
        let v = video_kbps(4.0, 30.0, 96).unwrap();
        let total_bytes = (v + 96) as f64 * 1000.0 / 8.0 * 30.0;
        assert!(total_bytes < 4.0 * 1024.0 * 1024.0);
        assert!(total_bytes > 3.5 * 1024.0 * 1024.0);
    }

    #[test]
    fn rejects_impossible_targets() {
        assert!(video_kbps(1.0, 120.0, 128).is_err());
        assert!(video_kbps(4.0, 0.0, 0).is_err());
    }

    #[test]
    fn suggests_downscale() {
        assert_eq!(suggested_height(300), Some(360));
        assert_eq!(suggested_height(1500), Some(720));
        assert_eq!(suggested_height(5000), None);
    }
}
