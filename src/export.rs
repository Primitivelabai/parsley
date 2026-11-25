use std::fmt::Write;
use std::path::Path;

use serde_json::{Value, json};

use crate::report::Session;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Html,
    Csv,
    Json,
}

impl Format {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("");
        match extension.to_ascii_lowercase().as_str() {
            "html" => Ok(Self::Html),
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            _ => Err("Choose an .html, .csv or .json output file.".into()),
        }
    }
}

pub fn render(format: Format, sessions: &[(&str, &Session)]) -> Result<String, String> {
    match format {
        Format::Html => Err("HTML is rendered by the report view.".into()),
        Format::Csv => Ok(csv(sessions)),
        Format::Json => serde_json::to_string_pretty(&json!({
            "schema": 1,
            "sessions": sessions.iter().map(|(name, session)| {
                let range = session.window.map(|(start, end)| {
                    json!({"start_utc": start.to_rfc3339(), "end_utc": end.to_rfc3339()})
                });
                json!({
                    "session": name,
                    "window": range,
                    "partial": !session.notices.is_empty(),
                    "notices": session.notices,
                    "events": session.events.iter().map(|event| {
                        json!({
                            "time_utc": event.at.to_rfc3339(),
                            "event": event.kind.title(),
                            "channel": event.kind.channel(),
                            "provider": event.kind.provider(),
                            "event_id": event.id,
                            "record_id": event.record,
                            "stop_code": event.detail.stop_code.map(|code| format!("0x{code:08X}")),
                            "exception_code": event.detail.exception.map(|code| format!("0x{code:08X}"))
                        })
                    }).collect::<Vec<Value>>(),
                })
            }).collect::<Vec<Value>>(),
        })).map_err(|error| format!("Could not encode the event export. {error}")),
    }
}

fn csv(sessions: &[(&str, &Session)]) -> String {
    let mut csv = String::from(
        "session,partial,time_utc,event,channel,provider,event_id,record_id,stop_code,exception_code\r\n",
    );
    for (name, session) in sessions {
        let partial = !session.notices.is_empty();
        for event in &session.events {
            // The textual columns are fixed Parsley labels and an ISO timestamp.
            // The file deliberately omits arbitrary values from the event payload.
            let _ = write!(
                csv,
                "{name},{partial},{},\"{}\",{},\"{}\",{},{},{},{}\r\n",
                event.at.to_rfc3339(),
                event.kind.title(),
                event.kind.channel(),
                event.kind.provider(),
                event.id,
                event.record,
                event
                    .detail
                    .stop_code
                    .map_or(String::new(), |code| format!("0x{code:08X}")),
                event
                    .detail
                    .exception
                    .map_or(String::new(), |code| format!("0x{code:08X}")),
            );
        }
    }
    csv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_exports_omit_application_names() {
        let demo = Session {
            events: crate::incidents::demo(),
            notices: vec!["Incomplete".into()],
            window: None,
        };
        let sessions = [("incident", &demo)];
        let json = render(Format::Json, &sessions).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["sessions"][0]["events"][1]["event_id"], 1000);
        assert_eq!(parsed["sessions"][0]["partial"], true);
        assert!(!json.contains("demo-editor"));
        let csv = render(Format::Csv, &sessions).unwrap();
        assert!(csv.contains("incident,true,"));
        assert!(csv.contains("0x0000009F"));
        assert!(!csv.contains("demo-renderer"));
    }
}
