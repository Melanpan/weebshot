use super::Request;
use anyhow::{Result, ensure};
use rmcp::model::Tool;
use serde_json::{Value, json};

fn specs() -> Vec<(&'static str, &'static str, Value, Vec<&'static str>)> {
    let number = json!({"type":"number","minimum":0});
    let integer = json!({"type":"integer","minimum":0});
    let string = json!({"type":"string","minLength":1});
    let boolean = json!({"type":"boolean"});
    let timeline = json!({"seconds":number,"frame":integer});
    let markers = json!({"in_seconds":number,"out_seconds":number,"replace_selection":boolean,"seek_to_in":boolean});
    let mut prepare = markers.clone();
    prepare["media"] = string.clone();
    prepare["settings"] = settings_schema();
    vec![
        (
            "video_timeline_scenes",
            "Discover approximate hard-cut shots using conventional FFmpeg scene scores. Returns start/end and midpoint representative timestamps, threshold and analysis settings. Analyzes every frame in the requested interval: use short candidate intervals after a uniform overview; fewer max_scenes or smaller thumbnails do not shorten analysis. Long scans may exceed the client's timeout (server limit defaults to 120 seconds). Optional thumbnail sheet uses the independent contact-sheet decoder. Does not modify GUI state or queue. Up to 1000 scenes; thumbnails must fit contact-sheet limits.",
            json!({
                "start_seconds":number,"end_seconds":number,"threshold":{"type":"number","minimum":0.01,"maximum":1,"default":0.35},
                "max_scenes":{"type":"integer","minimum":1,"maximum":1000,"default":100},"include_thumbnails":{"type":"boolean","default":false},
                "min_scene_seconds":{"type":"number","minimum":0.05,"maximum":10,"default":0.25},"frame_width":{"type":"integer","minimum":128,"maximum":4096,"default":240},"columns":{"type":"integer","minimum":1,"maximum":6,"default":4}
            }),
            vec![],
        ),
        (
            "report_issue",
            "Record an agent diagnostic event in existing Weebshot Logs (not a GitHub submission). Fields are sanitized and bounded, no automatic private data collected. Limit 5 accepted reports/minute per Weebshot process; returns report_id only after log acceptance.",
            json!({
                "title":{"type":"string","minLength":1,"maxLength":200},"description":{"type":"string","minLength":1,"maxLength":4096},
                "severity":{"enum":["debug","info","warning","error"]},"component":{"type":"string","maxLength":80},"tool_name":{"type":"string","maxLength":80},
                "error_message":{"type":"string","maxLength":2048},"reproduction_steps":{"type":"array","maxItems":16,"items":{"type":"string","maxLength":512}},
                "related_job_ids":{"type":"array","maxItems":32,"items":{"type":"integer","minimum":0}}
            }),
            vec!["title", "description"],
        ),
        (
            "video_contact_sheet",
            "Read-only timestamped contact sheet returned as a JPEG image with requested/actual timestamps and tile metadata. For unfamiliar footage: first use uniform for a coarse overview, then interval on short candidate ranges (e.g. 10 seconds, interval_seconds=0.25, frame_width=480, max_frames=40), then video_extract_frames at max_width=1280 or larger to compare expressions. A sparse whole-video overview cannot establish the best frame. Limits: 64 frames, 6 columns. Scene mode analyzes every frame in the interval and can exceed client timeouts; prefer uniform for episode overviews. Uses a separate decoder; never seeks playback or creates queue jobs. Uniform uses bin centers; interval reports truncation; scene uses scene midpoints with uniform fallback.",
            json!({
                "start_seconds":number,"end_seconds":number,"sampling":{"type":"string","enum":["interval","uniform","scene"],"default":"interval"},
                "interval_seconds":{"type":"number","minimum":0.001,"default":10,"description":"Temporal sampling density in interval mode. For detailed inspection use 0.25–1 second within a short candidate interval; exceeding max_frames truncates the end of the interval and reports it."},"columns":{"type":"integer","minimum":1,"maximum":6,"default":4},
                "frame_width":{"type":"integer","minimum":128,"maximum":4096,"default":480,"description":"Thumbnail width in pixels. Use 480 for scene details; smaller episode overviews are only for locating candidate regions. Full-image pixel limits also apply."},"max_frames":{"type":"integer","minimum":1,"maximum":64,"default":24,"description":"Maximum output tiles, not scene-analysis work. Narrow the time interval for denser inspection rather than using an oversized episode-wide grid."},
                "include_timestamps":{"type":"boolean","default":true},"threshold":{"type":"number","minimum":0.01,"maximum":1,"default":0.3}
            }),
            vec![],
        ),
        (
            "video_extract_frames",
            "Read-only batch of up to 16 detailed frames for comparing expressions and composition after locating a candidate with video_contact_sheet. Use closely spaced fractional-second timestamps within the candidate shot and max_width=1280 or larger; do not choose a final screenshot from tiny overview thumbnails alone. Returns indexed JPEG/PNG images and actual native presentation timestamps, preserving request order and partial decoding errors. Selects the first decodable frame at or after each timestamp (VFR-safe); use actual timestamps from results for the final screenshot. Does not modify the GUI or queue.",
            json!({
                "timestamps_seconds":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"number","minimum":0}},
                "max_width":{"type":"integer","minimum":128,"maximum":4096,"default":1280},"format":{"type":"string","enum":["jpeg","png"],"default":"jpeg"},
                "quality":{"type":"integer","minimum":1,"maximum":100,"default":85}
            }),
            vec!["timestamps_seconds"],
        ),
        (
            "media_open",
            "Open a local video. Requires replace_selection=true before discarding existing markers.",
            json!({"path":string,"replace_selection":boolean}),
            vec!["path"],
        ),
        (
            "media_info",
            "Read active media metadata, duration, resolution, codecs and tracks.",
            json!({}),
            vec![],
        ),
        (
            "media_tracks",
            "List media tracks. Track IDs are ffprobe stream indices in headless mode, mpv IDs in GUI mode.",
            json!({}),
            vec![],
        ),
        (
            "media_select_track",
            "Select audio/subtitles. Use exactly one of track_id (null disables) or external_path (subtitle file). id_namespace=native means mpv in GUI and ffprobe in headless; select ffprobe explicitly for stream indices. Video selection is unsupported.",
            json!({"kind":{"enum":["audio","subtitle"]},"track_id":{"type":["integer","null"],"minimum":0},"id_namespace":{"enum":["native","mpv","ffprobe"]},"external_path":string}),
            vec!["kind"],
        ),
        (
            "media_close",
            "Close media; requires replace_selection=true if markers exist.",
            json!({"replace_selection":boolean}),
            vec![],
        ),
        (
            "timeline_get_state",
            "Read active timeline, frame rate, playback state and markers.",
            json!({}),
            vec![],
        ),
        (
            "timeline_seek",
            "Seek to seconds or a zero-based frame (specify exactly one). Headless mode updates the extraction cursor.",
            timeline.clone(),
            vec![],
        ),
        (
            "timeline_set_in",
            "Set the In marker to seconds, or current position. Existing markers require replace_selection=true.",
            json!({"seconds":number,"replace_selection":boolean}),
            vec![],
        ),
        (
            "timeline_set_out",
            "Set the Out marker to seconds, or current position. Existing markers require replace_selection=true.",
            json!({"seconds":number,"replace_selection":boolean}),
            vec![],
        ),
        (
            "timeline_set_range",
            "Set an ordered In/Out range atomically. Existing markers require replace_selection=true.",
            markers.clone(),
            vec!["in_seconds", "out_seconds"],
        ),
        (
            "playback_control",
            "GUI: play, pause or step. Headless: pause and step update the cursor; play requires a GUI.",
            json!({"action":{"enum":["play","pause","step"]},"frames":{"type":"integer","minimum":-1000,"maximum":1000}}),
            vec!["action"],
        ),
        (
            "export_get_settings",
            "Read current export and capture settings, including existing quality presets.",
            json!({}),
            vec![],
        ),
        (
            "export_set_settings",
            "Patch existing export settings. Same settings and presets as the GUI; no executable paths or arbitrary ffmpeg arguments.",
            json!({"settings":settings_schema()}),
            vec!["settings"],
        ),
        (
            "screenshot_extract",
            "Queue exact-frame extraction using the existing screenshot pipeline. output must end in png/jpg/jpeg/webp. jobs_get can return a bounded PNG preview.",
            json!({"output":string,"base_directory":string,"seconds":number,"frame":integer,"subtitles":boolean,"crop":crop_schema()}),
            vec!["output"],
        ),
        (
            "export_add",
            "Queue a clip using current settings and markers, or explicit In/Out. Output extension must match format. Both subtitle mode queues the requested output plus a _raw sibling; returns job_id and job_ids.",
            json!({"output":string,"base_directory":string,"in_seconds":number,"out_seconds":number,"settings":settings_schema(),"crop":crop_schema()}),
            vec!["output"],
        ),
        (
            "stitch_create",
            "Queue the existing camera-pan Stitch engine, with markers or automatic boundaries. Saves PNG or editable layered PSD; no grid/collage layouts are implemented.",
            json!({"output":string,"base_directory":string,"in_seconds":number,"out_seconds":number,"method":{"enum":["automatic","translation","two_vertical_layers","shift_and_scale","layered_motion"]},"sampling_fps":{"type":"number","minimum":1,"maximum":30},"search_seconds":{"type":"number","minimum":0.1,"maximum":60},"layer_divider_percent":{"type":"integer","minimum":10,"maximum":90},"prefer_background":boolean,"force":boolean}),
            vec!["output"],
        ),
        (
            "jobs_list",
            "List this automation session's queued, active and completed jobs.",
            json!({}),
            vec![],
        ),
        (
            "jobs_get",
            "Read job progress/result. preview=true returns at most 512px PNG for completed PNG/JPEG screenshots and PNG Stitches.",
            json!({"job_id":integer,"preview":boolean}),
            vec!["job_id"],
        ),
        (
            "jobs_cancel",
            "Cancel a queued or active job, terminating its subprocess. Completed jobs cannot be cancelled.",
            json!({"job_id":integer}),
            vec!["job_id"],
        ),
        (
            "gui_get_state",
            "Read the attached GUI state, settings and automation jobs. Fails without a GUI.",
            json!({}),
            vec![],
        ),
        (
            "gui_open_media",
            "Open local media in an attached GUI.",
            json!({"path":string,"replace_selection":boolean}),
            vec!["path"],
        ),
        (
            "gui_seek",
            "Seek the visible GUI playhead.",
            timeline,
            vec![],
        ),
        (
            "gui_set_range",
            "Set visible GUI markers, with selection protection.",
            markers,
            vec!["in_seconds", "out_seconds"],
        ),
        (
            "gui_set_export_settings",
            "Update visible GUI export controls.",
            json!({"settings":settings_schema()}),
            vec!["settings"],
        ),
        (
            "gui_prepare_export",
            "Prepare media, selection and export settings for review; does not start exporting. Opening new media is asynchronous; read state before subsequent operations.",
            prepare,
            vec!["in_seconds", "out_seconds"],
        ),
    ]
}
fn crop_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"w":{"type":"integer","minimum":2},"h":{"type":"integer","minimum":2}},"required":["x","y","w","h"]})
}
pub fn settings_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{
        "clip_format":{"enum":["Webm","Mp4","Mkv","Gif","Webp"]},"quality":{"enum":["Small","Balanced","High"]},
        "clip_stream_copy":{"type":"boolean"},"include_audio":{"type":"boolean"},"clip_subs":{"enum":["Off","Burned","Both"]},
        "shot_subs":{"enum":["Off","Burned","Both"]},"shot_format":{"enum":["Png","Jpg","Webp"]},
        "use_target_size":{"type":"boolean"},"target_mb":{"type":"number","minimum":0.01,"maximum":100000},
        "max_height":{"type":["integer","null"],"minimum":2,"maximum":16384},"max_fps":{"type":["integer","null"],"minimum":1,"maximum":240},
        "anim_height":{"type":["integer","null"],"minimum":2,"maximum":16384},"webp_height":{"type":["integer","null"],"minimum":2,"maximum":16384},
        "anim_fps":{"type":"integer","minimum":1,"maximum":120},"loop_count":{"type":"integer","minimum":1,"maximum":100},
        "anim":{"type":"object","additionalProperties":false,"properties":{"colors":{"type":"integer","minimum":2,"maximum":256},"dither":{"enum":["None","Bayer","FloydSteinberg","Sierra"]},"dedupe":{"type":"boolean"},"rect_diff":{"type":"boolean"},"gifsicle_optimize":{"type":"boolean"},"gifsicle_lossy":{"type":"integer","minimum":0,"maximum":200}}},
        "encoder":{"type":"object","additionalProperties":false,"properties":{"x264_preset":{"enum":["ultrafast","superfast","veryfast","faster","fast","medium","slow","slower","veryslow"]},"vp9_cpu_used":{"type":"integer","minimum":0,"maximum":5},"hw":{"enum":[null,"Nvenc","Qsv","Amf"]}}},
        "clip_anime4k":anime_schema(),"shot_anime4k":anime_schema()
    }})
}
fn anime_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{"enabled":{"type":"boolean"},"model":{"enum":["AcNet","ArNet"]},"speed":{"enum":["Fast","Balanced","Slow"]},"variant":{"enum":["Neutral","Denoise","Lines","LinesDenoise"]},"device":{"enum":["Auto","Gpu","Cpu"]}}})
}
pub fn definitions() -> Vec<Tool> {
    specs().into_iter().map(|(name,description,properties,required)| {
        let mut schema=json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
        if name=="media_select_track" {schema["oneOf"]=json!([{"required":["track_id"]},{"required":["external_path"]}]);}
        serde_json::from_value(json!({"name":name,"description":description,"inputSchema":schema})).expect("static tool schema")
    }).collect()
}
pub fn validate(request: &Request) -> Result<()> {
    let tool = definitions()
        .into_iter()
        .find(|t| t.name == request.name)
        .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", request.name))?;
    validate_value(
        &request.arguments,
        &Value::Object((*tool.input_schema).clone()),
        "arguments",
    )
}
// The tool schemas deliberately use only this small, auditable JSON Schema subset.
fn validate_value(value: &Value, schema: &Value, path: &str) -> Result<()> {
    if let Some(variants) = schema["oneOf"].as_array() {
        ensure!(
            variants
                .iter()
                .filter(|s| validate_value(value, s, path).is_ok())
                .count()
                == 1,
            "{path}: specify exactly one of track_id (null disables) or external_path"
        );
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        ensure!(choices.contains(value), "{path}: unsupported value");
    }
    if let Some(ty) = schema.get("type") {
        let matches = |ty: &str| match ty {
            "array" => value.is_array(),
            "object" => value.is_object(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "null" => value.is_null(),
            _ => false,
        };
        ensure!(
            ty.as_str().is_some_and(matches)
                || ty
                    .as_array()
                    .is_some_and(|a| a.iter().any(|t| t.as_str().is_some_and(matches))),
            "{path}: incorrect type"
        );
    }
    if let Some(n) = value.as_f64() {
        ensure!(n.is_finite(), "{path}: must be finite");
        if let Some(min) = schema["minimum"].as_f64() {
            ensure!(n >= min, "{path}: must be >= {min}");
        }
        if let Some(max) = schema["maximum"].as_f64() {
            ensure!(n <= max, "{path}: must be <= {max}");
        }
    }
    if let Some(s) = value.as_str() {
        ensure!(!s.contains('\0'), "{path}: NUL is not permitted");
        if let Some(max) = schema["maxLength"].as_u64() {
            ensure!(
                s.chars().count() <= max as usize,
                "{path}: at most {max} characters"
            );
        }
        if schema.get("minLength").is_some() {
            ensure!(!s.is_empty(), "{path}: cannot be empty");
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(min) = schema["minItems"].as_u64() {
            ensure!(
                items.len() >= min as usize,
                "{path}: provide at least {min} items"
            );
        }
        if let Some(max) = schema["maxItems"].as_u64() {
            ensure!(items.len() <= max as usize, "{path}: at most {max} items");
        }
        if let Some(item_schema) = schema.get("items") {
            for (i, item) in items.iter().enumerate() {
                validate_value(item, item_schema, &format!("{path}[{i}]"))?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for key in required {
                let key = key.as_str().unwrap();
                ensure!(object.contains_key(key), "{path}: missing {key}");
            }
        }
        for (key, value) in object {
            if let Some(child) = schema["properties"].get(key) {
                validate_value(value, child, &format!("{path}.{key}"))?;
            } else {
                ensure!(
                    schema["additionalProperties"] != false,
                    "{path}: unknown field {key}"
                );
            }
        }
    }
    Ok(())
}
