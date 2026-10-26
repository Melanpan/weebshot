use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Sub,
}

/// One entry of mpv's `track-list`.
#[derive(Clone, Debug)]
pub struct Track {
    pub kind: TrackKind,
    /// mpv track id (what `aid`/`sid` take).
    pub id: i64,
    /// Stream index inside the file as ffmpeg numbers it (`-map 0:N`).
    pub ff_index: Option<i64>,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub codec: String,
    /// Set for tracks loaded from a separate file (e.g. a sidecar .ass).
    pub external: Option<PathBuf>,
    pub channels: Option<i64>,
}

const BITMAP_SUB_CODECS: &[&str] = &[
    "hdmv_pgs_subtitle",
    "pgs",
    "dvd_subtitle",
    "dvb_subtitle",
    "xsub",
];

impl Track {
    pub fn label(&self) -> String {
        let mut s = format!("#{}", self.id);
        if let Some(lang) = &self.lang {
            s += &format!(" [{lang}]");
        }
        if let Some(title) = &self.title {
            s += &format!(" {title}");
        }
        s += &format!(" ({}", self.codec);
        if let Some(ch) = self.channels {
            s += &format!(", {ch}ch");
        }
        if self.external.is_some() {
            s += ", external";
        }
        s + ")"
    }

    pub fn is_bitmap_sub(&self) -> bool {
        self.kind == TrackKind::Sub && BITMAP_SUB_CODECS.contains(&self.codec.as_str())
    }
}

/// ffmpeg's `subtitles` filter selects the stream by its index *among subtitle
/// streams* of the file, not by absolute stream index.
pub fn relative_sub_index(tracks: &[Track], sub: &Track) -> usize {
    tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Sub && t.external == sub.external)
        .filter(|t| t.ff_index < sub.ff_index)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(kind: TrackKind, id: i64, ff: i64, codec: &str) -> Track {
        Track {
            kind,
            id,
            ff_index: Some(ff),
            lang: None,
            title: None,
            codec: codec.into(),
            external: None,
            channels: None,
        }
    }

    #[test]
    fn relative_index_skips_other_kinds() {
        let tracks = vec![
            track(TrackKind::Video, 1, 0, "h264"),
            track(TrackKind::Audio, 1, 1, "aac"),
            track(TrackKind::Audio, 2, 2, "flac"),
            track(TrackKind::Sub, 1, 3, "ass"),
            track(TrackKind::Sub, 2, 4, "ass"),
        ];
        assert_eq!(relative_sub_index(&tracks, &tracks[3]), 0);
        assert_eq!(relative_sub_index(&tracks, &tracks[4]), 1);
    }

    #[test]
    fn detects_bitmap_subs() {
        assert!(track(TrackKind::Sub, 1, 2, "hdmv_pgs_subtitle").is_bitmap_sub());
        assert!(!track(TrackKind::Sub, 1, 2, "ass").is_bitmap_sub());
    }
}
