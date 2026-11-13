//! Explicit agent diagnostics. No automatic environment, file contents or secrets.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const EVENT: &str = "mcp.agent_issue_report";
struct RateLimit {
    accepted: VecDeque<Instant>,
}
impl RateLimit {
    fn accept(&mut self, now: Instant) -> Result<()> {
        while self
            .accepted
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            self.accepted.pop_front();
        }
        ensure!(
            self.accepted.len() < 5,
            "Issue report rate limit reached (5 per minute); wait before retrying"
        );
        self.accepted.push_back(now);
        Ok(())
    }
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control()
                || matches!(c,'\u{2028}'|'\u{2029}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
            {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}
pub fn accept(arguments: &Value) -> Result<Value> {
    static LIMIT: OnceLock<Mutex<RateLimit>> = OnceLock::new();
    super::tools::validate(&super::Request {
        name: "report_issue".into(),
        arguments: arguments.clone(),
    })?;
    ensure!(
        serde_json::to_vec(arguments)?.len() <= 16 * 1024,
        "Issue report exceeds the 16 KiB total limit; shorten its fields"
    );
    let mut fields = serde_json::Map::new();
    for key in [
        "title",
        "description",
        "component",
        "tool_name",
        "error_message",
    ] {
        if let Some(value) = arguments[key].as_str() {
            let value = clean(value);
            if matches!(key, "title" | "description") {
                ensure!(!value.is_empty(), "{key} must contain printable text");
            }
            fields.insert(key.into(), json!(value));
        }
    }
    fields.entry("component").or_insert(json!("mcp"));
    let steps = arguments["reproduction_steps"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| clean(v.as_str().unwrap()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    fields.insert("reproduction_steps".into(), json!(steps));
    fields.insert(
        "related_job_ids".into(),
        arguments
            .get("related_job_ids")
            .cloned()
            .unwrap_or(json!([])),
    );
    let level = match arguments["severity"].as_str().unwrap_or("warning") {
        "debug" => crate::log::Level::Debug,
        "info" => crate::log::Level::Info,
        "error" => crate::log::Level::Error,
        _ => crate::log::Level::Warn,
    };
    LIMIT
        .get_or_init(|| {
            Mutex::new(RateLimit {
                accepted: VecDeque::new(),
            })
        })
        .lock()
        .map_err(|_| anyhow::anyhow!("Diagnostic limiter unavailable"))?
        .accept(Instant::now())?;
    let id = crate::log::structured_event(level, "mcp", EVENT, fields)?;
    Ok(json!({"accepted":true,"event":EVENT,"report_id":id}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitizes_reports_and_uses_native_structured_log_fields() {
        let response=accept(&json!({"title":"test\nforged\u{001b}line","description":"diagnostic\ttext","reproduction_steps":["do not execute\ranything"],"severity":"error"})).unwrap();
        let id = response["report_id"].as_str().unwrap();
        crate::log::with_lines(|lines| {
            let line = lines
                .iter()
                .find(|l| l.fields.as_ref().is_some_and(|f| f["report_id"] == id))
                .unwrap();
            let fields = line.fields.as_ref().unwrap();
            assert_eq!(fields["event"], EVENT);
            assert_eq!(fields["title"], "test forged line");
            assert_eq!(fields["source"], "mcp");
            assert!(!line.text.contains(['\n', '\r', '\u{001b}']));
            assert!(line.timestamp_unix_ms > 0);
        });
    }
    #[test]
    fn bounded_report_schema_and_rate_window() {
        for args in [
            json!({"title":"x","description":"x","destination":"/tmp/no"}),
            json!({"title":"x","description":"x".repeat(4097)}),
            json!({"title":"x","description":"x","reproduction_steps":vec!["x";17]}),
        ] {
            assert!(
                super::super::tools::validate(&super::super::Request {
                    name: "report_issue".into(),
                    arguments: args
                })
                .is_err()
            );
        }
        let now = Instant::now();
        let mut limit = RateLimit {
            accepted: VecDeque::new(),
        };
        for _ in 0..5 {
            limit.accept(now).unwrap();
        }
        assert!(limit.accept(now).is_err());
        limit.accept(now + Duration::from_secs(60)).unwrap();
    }
}
