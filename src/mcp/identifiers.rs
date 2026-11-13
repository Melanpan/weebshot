//! Explicit track namespaces shared by GUI and headless metadata.
use crate::tracks::{Track, TrackKind};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub fn kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Sub => "subtitle",
    }
}
pub fn gui_tracks(tracks: &[Track], audio: Option<i64>, subtitle: Option<i64>) -> Value {
    json!(tracks.iter().map(|t|json!({"id":t.id,"mpv_id":t.id,"index":t.ff_index,"ffprobe_index":t.ff_index,"id_namespace":"mpv","type":kind_name(t.kind),"codec_type":kind_name(t.kind),"codec_name":t.codec,"language":t.lang,"title":t.title,"external":t.external,"selected":match t.kind{TrackKind::Video=>tracks.iter().find(|t|t.kind==TrackKind::Video).is_some_and(|v|v.id==t.id),TrackKind::Audio=>audio==Some(t.id),TrackKind::Sub=>subtitle==Some(t.id)}})).collect::<Vec<_>>())
}
pub fn gui_id(
    tracks: &[Track],
    kind: TrackKind,
    id: Option<i64>,
    namespace: &str,
) -> Result<Option<i64>> {
    let Some(id) = id else { return Ok(None) };
    let ffprobe = namespace == "ffprobe";
    let matches = tracks
        .iter()
        .filter(|t| {
            t.kind == kind
                && if ffprobe {
                    t.ff_index == Some(id)
                } else {
                    t.id == id
                }
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        bail!(
            "Stream index {id} is ambiguous across external tracks; select an explicit mpv_id with id_namespace=mpv"
        );
    }
    let valid = tracks
        .iter()
        .filter(|t| t.kind == kind)
        .filter_map(|t| {
            let n = if ffprobe { t.ff_index? } else { t.id };
            Some(format!("{n} ({})", t.label()))
        })
        .collect::<Vec<_>>()
        .join(", ");
    ensure!(
        !matches.is_empty(),
        "Track ID {id} is unavailable. Valid {} track IDs ({}): {}. Use id_namespace=ffprobe for stream indices or id_namespace=mpv for mpv_id",
        kind_name(kind),
        if ffprobe { "ffprobe" } else { "mpv" },
        if valid.is_empty() { "none" } else { &valid }
    );
    Ok(Some(matches[0].id))
}
pub fn headless_tracks(
    info: &Value,
    audio: Option<i64>,
    subtitle: Option<i64>,
    external: Option<&std::path::Path>,
) -> Value {
    let streams = info["streams"].as_array().cloned().unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .and_then(|s| s["index"].as_i64());
    let mut tracks = streams
        .into_iter()
        .map(|mut s| {
            let index = s["index"].clone();
            let kind = s["codec_type"].as_str().unwrap_or("");
            let selected = match kind {
                "video" => index.as_i64() == video,
                "audio" => index.as_i64() == audio,
                "subtitle" => index.as_i64() == subtitle && external.is_none(),
                _ => false,
            };
            let kind = kind.to_string();
            s["id"] = index.clone();
            s["ffprobe_index"] = index;
            s["mpv_id"] = Value::Null;
            s["id_namespace"] = json!("ffprobe");
            s["type"] = json!(kind);
            s["language"] = s["tags"]["language"].clone();
            s["title"] = s["tags"]["title"].clone();
            s["selected"] = json!(selected);
            s["external"] = Value::Null;
            s
        })
        .collect::<Vec<_>>();
    if let Some(path) = external {
        tracks.push(json!({"id":null,"mpv_id":null,"ffprobe_index":null,"index":null,"id_namespace":"external","type":"subtitle","codec_type":"subtitle","codec_name":path.extension().and_then(|s|s.to_str()),"external":path,"selected":true}));
    }
    json!(tracks)
}
pub fn headless_id(
    info: &Value,
    kind: &str,
    id: Option<i64>,
    namespace: &str,
) -> Result<Option<i64>> {
    ensure!(
        namespace != "mpv",
        "mpv track IDs are unavailable in headless mode; use id_namespace=ffprobe"
    );
    if let Some(id) = id {
        let tracks = info["streams"].as_array().cloned().unwrap_or_default();
        let valid = tracks
            .iter()
            .filter(|t| t["codec_type"] == kind)
            .map(|t| {
                format!(
                    "{} ({} {})",
                    t["index"],
                    t["tags"]["language"].as_str().unwrap_or("und"),
                    t["codec_name"].as_str().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        ensure!(
            tracks
                .iter()
                .any(|t| t["codec_type"] == kind && t["index"] == id),
            "Track ID {id} is unavailable. Valid {kind} track IDs (ffprobe): {}",
            if valid.is_empty() { "none" } else { &valid }
        );
    }
    Ok(id)
}
