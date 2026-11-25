use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::export::Format;
use crate::incidents::{Detail, Event, Kind};
use crate::report::{Session, Window};

const SCHEMA: u8 = 1;
const MAX_SOURCES: usize = 8;
const MAX_EVENTS: usize = 10_000;
const MAX_BOOKMARKS: usize = 50;
const MAX_CASE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DEBUGGER_BYTES: u64 = 2 * 1024 * 1024;
// Windows writes the stop record after the restart, so it can trail the crash
// by the dump write and boot. Allow a little clock skew before the crash.
const MATCH_BEFORE: TimeDelta = TimeDelta::minutes(3);
const MATCH_AFTER: TimeDelta = TimeDelta::minutes(15);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    schema: u8,
    start_utc: String,
    end_utc: String,
    fictional: bool,
    pub sources: Vec<Source>,
    pub bookmarks: Vec<RecordRef>,
    pub debugger: Option<Debugger>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub partial: bool,
    pub records: Vec<CaseEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseEvent {
    pub at_utc: String,
    pub kind: Kind,
    pub event_id: u16,
    pub record_id: u64,
    pub stop_code: Option<u32>,
    pub exception_code: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRef {
    pub source: usize,
    pub channel: Channel,
    pub record: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Channel {
    System,
    Application,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Application => "Application",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        if value.eq_ignore_ascii_case("system") {
            Ok(Self::System)
        } else if value.eq_ignore_ascii_case("application") {
            Ok(Self::Application)
        } else {
            Err("Choose System or Application for a bookmark.".into())
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Debugger {
    pub at_utc: String,
    pub stop_code: u32,
    pub module: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Correlation {
    Unmatched,
    Ambiguous,
    Candidate(RecordRef),
}

impl RecordRef {
    fn parse(value: &str) -> Result<Self, String> {
        let mut fields = value.split(':');
        let invalid = "Use SOURCE:CHANNEL:RECORD for each bookmark, such as 1:System:42.";
        let source = fields
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or(invalid)?;
        let channel = Channel::parse(fields.next().ok_or(invalid)?)?;
        let record = fields
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or(invalid)?;
        if fields.next().is_some() || source == 0 || record == 0 {
            return Err(invalid.into());
        }
        Ok(Self {
            source,
            channel,
            record,
        })
    }

    pub fn anchor(&self) -> String {
        format!(
            "source-{}-{}-{}",
            self.source,
            self.channel.name(),
            self.record
        )
    }
}

impl From<&Event> for CaseEvent {
    fn from(event: &Event) -> Self {
        Self {
            at_utc: event.at.to_rfc3339(),
            kind: event.kind,
            event_id: event.id,
            record_id: event.record,
            stop_code: event.detail.stop_code,
            exception_code: event.detail.exception,
        }
    }
}

impl CaseEvent {
    pub fn event(&self) -> Result<Event, String> {
        let at = crate::timestamp(&self.at_utc, "event time")?;
        if self.record_id == 0
            || Kind::from_event(self.kind.provider(), self.event_id) != Some(self.kind)
        {
            return Err("The case contains an invalid Windows record reference.".into());
        }
        Ok(Event {
            at,
            record: self.record_id,
            kind: self.kind,
            id: self.event_id,
            detail: Detail {
                stop_code: self.stop_code,
                exception: self.exception_code,
                ..Default::default()
            },
        })
    }

    pub fn reference(&self, source: usize) -> RecordRef {
        RecordRef {
            source,
            channel: if self.kind.channel() == "System" {
                Channel::System
            } else {
                Channel::Application
            },
            record: self.record_id,
        }
    }
}

impl From<Session> for Source {
    fn from(session: Session) -> Self {
        Source {
            partial: !session.notices.is_empty(),
            records: session.events.iter().map(CaseEvent::from).collect(),
        }
    }
}

impl Case {
    fn new(
        window: Window,
        sources: Vec<Source>,
        bookmarks: Vec<RecordRef>,
        debugger: Option<Debugger>,
    ) -> Self {
        Self {
            schema: SCHEMA,
            start_utc: window.0.to_rfc3339(),
            end_utc: window.1.to_rfc3339(),
            fictional: false,
            sources,
            bookmarks,
            debugger,
        }
    }

    pub fn fictional(&self) -> bool {
        self.fictional
    }

    pub fn window(&self) -> Result<Window, String> {
        Ok((
            crate::timestamp(&self.start_utc, "window start")?,
            crate::timestamp(&self.end_utc, "window end")?,
        ))
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err("Unsupported incident case version.".into());
        }
        let (start, end) = self.window()?;
        if end <= start || end - start > TimeDelta::hours(6) {
            return Err("The case must have a time window of up to six hours.".into());
        }
        if self.sources.is_empty()
            || self.sources.len() > MAX_SOURCES
            || self
                .sources
                .iter()
                .map(|source| source.records.len())
                .sum::<usize>()
                > MAX_EVENTS
            || self.bookmarks.len() > MAX_BOOKMARKS
        {
            return Err(
                "The case exceeds its source, event or bookmark limits. Choose a narrower window."
                    .into(),
            );
        }
        let mut records = HashSet::new();
        for (index, source) in self.sources.iter().enumerate() {
            for record in &source.records {
                if !(start..=end).contains(&record.event()?.at)
                    || !records.insert(record.reference(index + 1))
                {
                    return Err(
                        "The case contains an out-of-window or duplicate record in one source."
                            .into(),
                    );
                }
            }
        }
        let mut marked = HashSet::new();
        for bookmark in &self.bookmarks {
            if !records.contains(bookmark) || !marked.insert(bookmark) {
                return Err("A bookmark is duplicated or points outside the case.".into());
            }
        }
        if let Some(debugger) = &self.debugger {
            if !((start..=end)
                .contains(&crate::timestamp(&debugger.at_utc, "debugger crash time")?))
            {
                return Err("The debugger crash time must fall inside the case window.".into());
            }
            if debugger.stop_code == 0
                || debugger
                    .module
                    .as_deref()
                    .is_some_and(|value| !valid_module(value))
            {
                return Err("The case contains invalid debugger evidence.".into());
            }
        }
        Ok(())
    }

    pub fn correlation(&self, debugger: &Debugger) -> Result<Correlation, String> {
        let crash = crate::timestamp(&debugger.at_utc, "debugger crash time")?;
        let mut one_match = None;
        for (index, source) in self.sources.iter().enumerate() {
            for record in &source.records {
                if record.kind == Kind::Bugcheck
                    && record.stop_code == Some(debugger.stop_code)
                    && (-MATCH_BEFORE..=MATCH_AFTER).contains(&(record.event()?.at - crash))
                {
                    if one_match.is_some() {
                        return Ok(Correlation::Ambiguous);
                    }
                    one_match = Some(record.reference(index + 1));
                }
            }
        }
        Ok(one_match.map_or(Correlation::Unmatched, Correlation::Candidate))
    }
}

fn valid_module(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !value.contains("..")
}

fn parse_debugger(text: &str, at: DateTime<Utc>, include_module: bool) -> Result<Debugger, String> {
    let mut stop_code = None;
    let mut module = None;
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("BUGCHECK_CODE:") {
            let value = value.trim();
            let hex = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
                .unwrap_or(value);
            let code = u32::from_str_radix(hex, 16)
                .ok()
                .filter(|code| *code != 0)
                .ok_or("The debugger report contains an invalid BUGCHECK_CODE.")?;
            if stop_code
                .replace(code)
                .is_some_and(|previous| previous != code)
            {
                return Err("The debugger report contains competing stop codes. Select the output for one dump.".into());
            }
        }
        if include_module
            && let Some(value) = line
                .strip_prefix("MODULE_NAME:")
                .map(str::trim)
                .filter(|value| valid_module(value))
            && module
                .replace(value)
                .is_some_and(|previous| !previous.eq_ignore_ascii_case(value))
        {
            return Err("The debugger report contains competing module names. Select the output for one dump.".into());
        }
    }
    Ok(Debugger {
        at_utc: at.to_rfc3339(),
        stop_code: stop_code
            .ok_or("The report needs a WinDbg BUGCHECK_CODE field from a Windows stop error.")?,
        module: module.map(str::to_owned),
    })
}

fn read_limited(path: &Path, max: u64, what: &str) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    File::open(path)
        .and_then(|file| file.take(max + 1).read_to_end(&mut data))
        .map_err(|error| format!("Could not read {what}. {error}"))?;
    if data.len() as u64 > max {
        return Err(format!("The {what} is too large."));
    }
    Ok(data)
}

fn debugger_file(path: &Path, at: DateTime<Utc>, include_module: bool) -> Result<Debugger, String> {
    let bytes = read_limited(path, MAX_DEBUGGER_BYTES, "debugger report")?;
    let encoding_error = "Use a UTF-8 or UTF-16LE WinDbg text report.";
    let text = if let Some(utf16) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let (pairs, extra) = utf16.as_chunks::<2>();
        if !extra.is_empty() {
            return Err(encoding_error.into());
        }
        String::from_utf16(
            &pairs
                .iter()
                .copied()
                .map(u16::from_le_bytes)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| encoding_error)?
    } else {
        String::from_utf8(bytes).map_err(|_| encoding_error)?
    };
    parse_debugger(text.trim_start_matches('\u{feff}'), at, include_module)
}

#[derive(Default)]
struct Options {
    at: Option<DateTime<Utc>>,
    minutes: Option<i64>,
    sources: Vec<PathBuf>,
    debugger: Option<PathBuf>,
    debugger_at: Option<DateTime<Utc>>,
    include_module: bool,
    bookmarks: Vec<RecordRef>,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self::default();
        while let Some(flag) = args.next() {
            if flag == "--include-module" && !options.include_module {
                options.include_module = true;
                continue;
            }
            let value = args
                .next()
                .ok_or("Expected a case option followed by a value.")?;
            let value_text = || value.to_str().ok_or("The case option must use valid text.");
            match flag.to_str() {
                Some("--at") if options.at.is_none() => {
                    options.at = Some(crate::timestamp(value_text()?, "--at")?)
                }
                Some("--minutes") if options.minutes.is_none() => {
                    options.minutes = Some(
                        value_text()?
                            .parse::<i64>()
                            .ok()
                            .filter(|n| (1..=180).contains(n))
                            .ok_or("Set --minutes to 1–180.")?,
                    );
                }
                Some("--evtx") if options.sources.len() < MAX_SOURCES => {
                    let source = crate::log_path(PathBuf::from(value))?;
                    if options.sources.contains(&source) {
                        return Err("Each .evtx source must be selected once.".into());
                    }
                    options.sources.push(source);
                }
                Some("--debugger-report") if options.debugger.is_none() => {
                    options.debugger = Some(PathBuf::from(value))
                }
                Some("--debugger-at") if options.debugger_at.is_none() => {
                    options.debugger_at = Some(crate::timestamp(value_text()?, "--debugger-at")?);
                }
                Some("--bookmark") if options.bookmarks.len() < MAX_BOOKMARKS => {
                    options.bookmarks.push(RecordRef::parse(value_text()?)?);
                }
                _ => {
                    return Err(
                        "Unknown, repeated or excessive case option. See parsley --help.".into(),
                    );
                }
            }
        }
        Ok(options)
    }

    fn collect(self) -> Result<Case, String> {
        let at = self.at.ok_or("Choose the incident time with --at.")?;
        let window = crate::window(at, TimeDelta::minutes(self.minutes.unwrap_or(10)))?;
        let debugger = match (self.debugger, self.debugger_at) {
            (Some(path), Some(time)) => Some(debugger_file(&path, time, self.include_module)?),
            (None, None) if !self.include_module => None,
            _ => {
                return Err(
                    "Provide both --debugger-report and --debugger-at for debugger evidence."
                        .into(),
                );
            }
        };
        let sources = if self.sources.is_empty() {
            vec![Source::from(crate::collect(None, Some(window))?)]
        } else {
            let mut sources = Vec::with_capacity(self.sources.len());
            for (index, path) in self.sources.iter().enumerate() {
                let session = crate::collect(Some(path), Some(window))
                    .map_err(|error| format!("Could not import source {}. {error}", index + 1))?;
                sources.push(Source::from(session));
            }
            sources
        };
        Ok(Case::new(window, sources, self.bookmarks, debugger))
    }
}

fn demo_case() -> Case {
    let events = crate::incidents::demo();
    let restart = &events[3];
    let window = crate::window(restart.at, TimeDelta::minutes(10)).expect("valid demo window");
    let records = |channel| {
        events
            .iter()
            .filter(|event| {
                event.kind.channel() == channel && (window.0..=window.1).contains(&event.at)
            })
            .map(CaseEvent::from)
            .collect()
    };
    let mut case = Case::new(
        window,
        vec![
            Source {
                partial: false,
                records: records("System"),
            },
            Source {
                partial: false,
                records: records("Application"),
            },
        ],
        vec![RecordRef {
            source: 1,
            channel: Channel::System,
            record: restart.record,
        }],
        Some(Debugger {
            at_utc: restart.at.to_rfc3339(),
            stop_code: 0x9f,
            module: Some("demo_filter".into()),
        }),
    );
    case.fictional = true;
    case
}

pub fn run(command: &str, mut args: impl Iterator<Item = OsString>) -> Result<String, String> {
    let input = PathBuf::from(args.next().ok_or("Choose an incident case .json file.")?);
    if Format::from_path(&input)? != Format::Json {
        return Err("Incident cases use a .json file.".into());
    }
    if command == "case-report" {
        let report = PathBuf::from(args.next().ok_or("Choose a new HTML report path.")?);
        if args.next().is_some() || Format::from_path(&report)? != Format::Html {
            return Err("Use case-report CASE.json REPORT.html.".into());
        }
        let case = read_case(&input)?;
        let html = crate::report::render_case(&case)?;
        crate::write_report(&report, &html)?;
        return Ok(format!(
            "Saved redacted incident report to {}",
            report.display()
        ));
    }
    if command == "case-bookmark" {
        let output = PathBuf::from(args.next().ok_or("Choose a new incident case path.")?);
        let mut case = read_case(&input)?;
        let previous = case.bookmarks.len();
        for value in args {
            if case.bookmarks.len() >= MAX_BOOKMARKS {
                return Err("A case can hold up to 50 bookmarks.".into());
            }
            case.bookmarks.push(RecordRef::parse(
                value.to_str().ok_or("Invalid bookmark.")?,
            )?);
        }
        if case.bookmarks.len() == previous {
            return Err("Choose at least one record to bookmark.".into());
        }
        return save_case(&output, &case);
    }
    let case = if command == "case-create" {
        Options::parse(args)?.collect()?
    } else {
        if args.next().is_some() {
            return Err("Use case-demo CASE.json.".into());
        }
        demo_case()
    };
    save_case(&input, &case)
}

fn read_case(input: &Path) -> Result<Case, String> {
    let bytes = read_limited(input, MAX_CASE_BYTES, "incident case")?;
    let case: Case = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not decode the incident case. {error}"))?;
    case.validate()?;
    Ok(case)
}

fn save_case(output: &Path, case: &Case) -> Result<String, String> {
    if Format::from_path(output)? != Format::Json {
        return Err("Incident cases use a .json file.".into());
    }
    case.validate()?;
    let json = serde_json::to_string_pretty(case).map_err(|error| error.to_string())?;
    if json.len() as u64 > MAX_CASE_BYTES {
        return Err("The case is too large. Choose a narrower time window.".into());
    }
    crate::write_report(output, &json)?;
    Ok(format!("Saved the incident case to {}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debugger_import_only_keeps_selected_fields() {
        let at = crate::timestamp("2026-11-22T06:14:00Z", "test").unwrap();
        let content = "\u{feff}BUGCHECK_CODE: 9f\nMODULE_NAME: demo_filter\n\
                       STACK_TEXT: ffffe000 0000ab12\nPROCESS_NAME: private.exe\n\
                       IMAGE_NAME: C:\\Users\\Someone\\secret.sys\n";
        let file = tempfile::NamedTempFile::new().unwrap();
        let utf16: Vec<_> = content.encode_utf16().flat_map(u16::to_le_bytes).collect();
        std::fs::write(file.path(), utf16).unwrap();
        let redacted = debugger_file(file.path(), at, false).unwrap();
        assert_eq!(redacted.stop_code, 0x9f);
        assert_eq!(redacted.module, None);
        let with_module = debugger_file(file.path(), at, true).unwrap();
        assert_eq!(with_module.module.as_deref(), Some("demo_filter"));
        let bundle = serde_json::to_string(&with_module).unwrap();
        for private in [
            "Users",
            "secret.sys",
            "private.exe",
            "ffffe000",
            "IMAGE_NAME",
        ] {
            assert!(!bundle.contains(private));
        }
        assert!(parse_debugger("BUGCHECK_CODE: 9f\nBUGCHECK_CODE: d1", at, false).is_err());
        assert!(
            parse_debugger(
                "BUGCHECK_CODE: 9f\nMODULE_NAME: a\nMODULE_NAME: b",
                at,
                true
            )
            .is_err()
        );
        let malformed = parse_debugger(
            "BUGCHECK_CODE: 9f\nMODULE_NAME: C:\\secret\\driver",
            at,
            true,
        )
        .unwrap();
        assert!(malformed.module.is_none());
    }

    #[test]
    fn matching_abstains_when_evidence_conflicts_or_is_ambiguous() {
        let mut case = demo_case();
        let debugger = case.debugger.take().unwrap();
        let Correlation::Candidate(candidate) = case.correlation(&debugger).unwrap() else {
            panic!("expected a candidate");
        };
        assert_eq!(candidate.source, 1);
        let mut conflicting = Debugger {
            stop_code: 0xd1,
            at_utc: debugger.at_utc.clone(),
            module: None,
        };
        assert_eq!(
            case.correlation(&conflicting).unwrap(),
            Correlation::Unmatched
        );
        conflicting.stop_code = debugger.stop_code;
        let logged = case.sources[0]
            .records
            .iter()
            .find(|event| event.kind == Kind::Bugcheck)
            .unwrap()
            .event()
            .unwrap()
            .at;
        // A crash after the stop record was logged cannot be the same stop.
        conflicting.at_utc = (logged + TimeDelta::minutes(4)).to_rfc3339();
        assert_eq!(
            case.correlation(&conflicting).unwrap(),
            Correlation::Unmatched
        );
        let repeated = case.sources[0]
            .records
            .iter()
            .find(|event| event.kind == Kind::Bugcheck)
            .unwrap()
            .event()
            .unwrap();
        // Distinct source IDs preserve both records even if their exports overlap.
        case.sources[1].records.push(CaseEvent::from(&repeated));
        case.validate().unwrap();
        assert_eq!(case.correlation(&debugger).unwrap(), Correlation::Ambiguous);
    }

    #[test]
    fn matching_allows_the_stop_record_to_trail_the_crash() {
        let mut case = demo_case();
        let debugger = case.debugger.take().unwrap();
        let logged = case.sources[0]
            .records
            .iter()
            .find(|event| event.kind == Kind::Bugcheck)
            .unwrap()
            .event()
            .unwrap()
            .at;
        let at = |offset: i64| Debugger {
            at_utc: (logged - TimeDelta::minutes(offset)).to_rfc3339(),
            stop_code: debugger.stop_code,
            module: None,
        };
        // Stop record logged 10 minutes after the crash (slow dump and boot).
        assert!(matches!(
            case.correlation(&at(10)).unwrap(),
            Correlation::Candidate(_)
        ));
        assert!(matches!(
            case.correlation(&at(15)).unwrap(),
            Correlation::Candidate(_)
        ));
        assert!(matches!(
            case.correlation(&at(-3)).unwrap(),
            Correlation::Candidate(_)
        ));
        assert_eq!(case.correlation(&at(16)).unwrap(), Correlation::Unmatched);
        assert_eq!(case.correlation(&at(-4)).unwrap(), Correlation::Unmatched);
    }

    #[test]
    fn case_round_trip_retains_evidence_links_and_discards_private_details() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("case.json");
        let booked = dir.path().join("bookmarked.json");
        let report = dir.path().join("report.html");
        run("case-demo", [bundle.clone().into_os_string()].into_iter()).unwrap();
        run(
            "case-bookmark",
            [
                bundle.clone().into_os_string(),
                booked.clone().into_os_string(),
                "2:Application:2".into(),
            ]
            .into_iter(),
        )
        .unwrap();
        let data = std::fs::read_to_string(&booked).unwrap();
        assert!(!data.contains("demo-editor.exe"));
        assert!(!data.contains("demo-renderer.dll"));
        run(
            "case-report",
            [booked.into_os_string(), report.clone().into_os_string()].into_iter(),
        )
        .unwrap();
        let html = std::fs::read_to_string(report).unwrap();
        assert!(html.contains("Fictional incident case"));
        assert!(html.contains("Candidate Windows stop record"));
        for anchor in [
            "source-1-System-4",
            "source-1-System-5",
            "source-2-Application-2",
        ] {
            assert!(html.contains(&format!("href=\"#{}\"", anchor)));
            assert!(html.contains(&format!("id=\"{}\"", anchor)));
        }
        assert!(!html.contains("demo-editor.exe"));
        assert!(
            run(
                "case-bookmark",
                [
                    bundle.clone().into_os_string(),
                    bundle.into_os_string(),
                    "9:System:1".into()
                ]
                .into_iter()
            )
            .is_err()
        );
    }
}
