//! Groups recurring incidents from crash history and keeps the Windows evidence
//! behind every group. The desktop view and the HTML report both render the
//! same `Analysis`, so their counts and evidence cannot drift apart.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;

use crate::capture::{Capture, METRICS};
use crate::incidents::{Detail, Event, HelpArticle, Kind};
use crate::versions::{GROUPS, Snapshot};

/// Optional local evidence that can show what changed around an incident.
#[derive(Default, Clone, Copy)]
pub struct Clues<'a> {
    pub health: Option<&'a Capture>,
    pub versions: Option<(&'a Snapshot, &'a Snapshot)>,
}

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub incidents: usize,
    pub groups: Vec<Group>,
    pub notices: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub key: String,
    /// `signature`, `stop-code`, `unexplained` or `evidence-poor`.
    pub basis: &'static str,
    pub title: String,
    pub explanation: String,
    pub evidence: Vec<String>,
    pub next_check: String,
    pub(crate) help: Option<HelpArticle>,
    pub incidents: usize,
    pub first: String,
    pub last: String,
    pub frequency: Frequency,
    pub records: Vec<Record>,
    pub clues: Vec<String>,
}

/// Incidents per UTC day, oldest first, ending on the analysis day.
#[derive(Debug, Serialize)]
pub struct Frequency {
    pub days7: Vec<usize>,
    pub days30: Vec<usize>,
    pub days90: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub at: String,
    pub record: String,
    pub id: u16,
    pub channel: &'static str,
    pub provider: &'static str,
    pub title: &'static str,
    pub explanation: &'static str,
    pub next_step: &'static str,
    pub(crate) help: Option<HelpArticle>,
    pub application: Option<String>,
    pub module: Option<String>,
    pub exception: Option<u32>,
    pub stop_code: Option<u32>,
    /// Set when this record is half of a linked stop-error and restart pair.
    pub linked: Option<String>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Crash(Kind, String, Option<String>, Option<u32>),
    Stop(u32),
    Unexplained,
    Poor(Kind),
    Other(Kind),
}

struct Incident<'a> {
    events: Vec<&'a Event>,
}

impl Incident<'_> {
    fn at(&self) -> DateTime<Utc> {
        self.events
            .iter()
            .map(|event| event.at)
            .min()
            .expect("an incident has events")
    }
}

/// Stable pseudonyms so redacted exports and the redacted desktop view agree.
struct Names {
    include: bool,
    applications: HashMap<String, String>,
    modules: HashMap<String, String>,
}

impl Names {
    fn new(events: &[Event], include: bool) -> Self {
        let mut names = Self {
            include,
            applications: HashMap::new(),
            modules: HashMap::new(),
        };
        let mut ordered: Vec<_> = events.iter().collect();
        ordered.sort_by_key(|event| (event.at, event.record));
        for event in ordered {
            if let Some(application) = &event.detail.application {
                let next = label("Application", names.applications.len());
                names
                    .applications
                    .entry(application.clone())
                    .or_insert(next);
            }
            if let Some(module) = &event.detail.module {
                let next = label("Module", names.modules.len());
                names.modules.entry(module.clone()).or_insert(next);
            }
        }
        names
    }

    fn application(&self, value: &str) -> String {
        if self.include {
            value.into()
        } else {
            self.applications[value].clone()
        }
    }

    fn module(&self, value: &str) -> String {
        if self.include {
            value.into()
        } else {
            self.modules[value].clone()
        }
    }
}

fn label(prefix: &str, index: usize) -> String {
    let mut letters = String::new();
    let mut index = index;
    loop {
        letters.insert(0, char::from(b'A' + (index % 26) as u8));
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    format!("{prefix} {letters}")
}

pub fn hex(value: u32) -> String {
    format!("0x{value:08X}")
}

pub fn analyze(
    events: &[Event],
    now: DateTime<Utc>,
    include_names: bool,
    clues: Clues,
) -> Analysis {
    let names = Names::new(events, include_names);
    let events: Vec<&Event> = events
        .iter()
        .filter(|event| event.kind.is_incident())
        .collect();
    let owned: Vec<Event> = events.iter().map(|event| (*event).clone()).collect();
    let mut paired_stop = BTreeMap::new();
    for event in &events {
        if let Some(record) = event.nearby_stop_error(&owned) {
            paired_stop.insert(record, *event);
        }
    }
    let mut incidents: BTreeMap<Key, Vec<Incident>> = BTreeMap::new();
    for event in &events {
        let detail = &event.detail;
        let key = match event.kind {
            Kind::Restart => {
                if paired_stop
                    .values()
                    .any(|restart| restart.record == event.record)
                {
                    continue;
                }
                Key::Unexplained
            }
            Kind::Bugcheck => match detail.stop_code {
                Some(code) => Key::Stop(code),
                None => Key::Poor(Kind::Bugcheck),
            },
            Kind::Crash | Kind::Hang => match &detail.application {
                Some(application) => Key::Crash(
                    event.kind,
                    application.clone(),
                    detail.module.clone(),
                    detail.exception,
                ),
                None => Key::Poor(event.kind),
            },
            kind => Key::Other(kind),
        };
        let mut members = vec![*event];
        if event.kind == Kind::Bugcheck
            && let Some(restart) = paired_stop.get(&event.record)
        {
            members.insert(0, *restart);
        }
        incidents
            .entry(key)
            .or_default()
            .push(Incident { events: members });
    }
    let total = incidents.values().map(Vec::len).sum();
    let mut groups: Vec<Group> = incidents
        .iter()
        .map(|(key, list)| build(key, list, &incidents, &events, &names, now, clues))
        .collect();
    groups.sort_by(|a, b| {
        let poor = |group: &Group| matches!(group.basis, "unexplained" | "evidence-poor");
        poor(a)
            .cmp(&poor(b))
            .then(b.incidents.cmp(&a.incidents))
            .then(b.last.cmp(&a.last))
    });
    for (index, group) in groups.iter_mut().enumerate() {
        group.key = format!("group-{}", index + 1);
    }
    let mut notices = Vec::new();
    if !include_names && !events.is_empty() {
        notices.push(
            "Application and module names are hidden. Groups still use them, so counts match the named view."
                .into(),
        );
    }
    if let Some((before, after)) = clues.versions
        && !groups.iter().any(|group| bracketed(group, before, after))
    {
        notices.push(
            "The attached inventory snapshots were not taken on either side of any group, so they show no change around an incident."
                .into(),
        );
    }
    Analysis {
        incidents: total,
        groups,
        notices,
    }
}

fn build(
    key: &Key,
    list: &[Incident],
    all: &BTreeMap<Key, Vec<Incident>>,
    events: &[&Event],
    names: &Names,
    now: DateTime<Utc>,
    clues: Clues,
) -> Group {
    let first = list
        .iter()
        .map(Incident::at)
        .min()
        .expect("group has incidents");
    let last = list
        .iter()
        .map(Incident::at)
        .max()
        .expect("group has incidents");
    let anchor = list
        .iter()
        .flat_map(|incident| incident.events.iter())
        .max_by_key(|event| (event.at, event.record))
        .expect("group has events");
    let mut evidence = Vec::new();
    let count = list.len();
    let times = if count == 1 {
        "once".to_owned()
    } else {
        format!("{count} times")
    };
    let (basis, title, explanation, next_check) = match key {
        Key::Crash(kind, application, module, exception) => {
            let application = names.application(application);
            let verb = if *kind == Kind::Hang {
                "stopped responding"
            } else {
                "crashed"
            };
            evidence.push(format!("Application recorded by Windows: {application}"));
            match module {
                Some(module) => evidence.push(format!("Faulting module: {}", names.module(module))),
                None if *kind == Kind::Crash => {
                    evidence.push("Windows did not record a faulting module.".into())
                }
                None => {}
            }
            match exception {
                Some(code) => evidence.push(format!("Exception code: {}", hex(*code))),
                None if *kind == Kind::Crash => {
                    evidence.push("Windows did not record an exception code.".into())
                }
                None => {}
            }
            let others = all
                .keys()
                .filter(|other| {
                    matches!(other, Key::Crash(k, a, _, _)
                        if *k == *kind && names.application(a) == application && *other != key)
                })
                .count();
            if others > 0 {
                evidence.push(format!(
                    "The same application {verb} with {others} other signature{}, grouped separately because the module or exception code differs.",
                    if others == 1 { "" } else { "s" }
                ));
            }
            let mut title = format!("{application} {verb}");
            if let Some(module) = module {
                title.push_str(&format!(" in {}", names.module(module)));
            }
            let explanation = if *kind == Kind::Hang {
                format!("Windows recorded {application} as not responding {times}.")
            } else {
                format!(
                    "Windows recorded the same crash signature {times}. The faulting module is where Windows observed the failure, which is not always its cause."
                )
            };
            let next = if *kind == Kind::Hang {
                "Check whether this app freezes again while you do the same thing.".to_owned()
            } else {
                "Check whether the same component and error code appear each time the app crashes."
                    .to_owned()
            };
            ("signature", title, explanation, next)
        }
        Key::Stop(code) => {
            let name = Detail::stop_name(*code)
                .map(|name| format!(" ({name})"))
                .unwrap_or_default();
            evidence.push(format!(
                "Stop code recorded by Windows: {}{name}",
                hex(*code)
            ));
            let pairs = list
                .iter()
                .filter(|incident| incident.events.len() > 1)
                .count();
            if pairs > 0 {
                evidence.push(format!(
                    "{pairs} of these stop errors {} followed by exactly one unexpected-restart record within 3 minutes and no competing restart, so each pair counts as one incident.",
                    if pairs == 1 { "was" } else { "were" }
                ));
            }
            (
                "stop-code",
                format!("Windows stop error {}{name}", hex(*code)),
                format!(
                    "Windows recorded a stop error with this code {times}. The stop code names the kind of failure, not the component that caused it."
                ),
                "Keep the stop code and check whether it appears again if Windows crashes.".into(),
            )
        }
        Key::Unexplained => {
            evidence.push(
                "Kernel-Power Event 41 records an unclean shutdown. It does not record why.".into(),
            );
            let ambiguous = list
                .iter()
                .filter(|incident| {
                    let restart = incident.events[0];
                    events.iter().any(|event| {
                        event.kind == Kind::Bugcheck
                            && event.at >= restart.at
                            && event.at - restart.at <= TimeDelta::minutes(3)
                    })
                })
                .count();
            if ambiguous > 0 {
                evidence.push(format!(
                    "{ambiguous} of these restarts had a stop error logged nearby, but more than one restart or stop error could match, so the records do not establish which restart it belongs to."
                ));
            }
            (
                "unexplained",
                "Unexplained restart".into(),
                format!(
                    "Windows started after an unclean shutdown {times}. The cause stays undetermined until a record corroborates it."
                ),
                "Look at the records around each restart for a stop error or hardware error."
                    .into(),
            )
        }
        Key::Poor(kind) => {
            let missing = if *kind == Kind::Bugcheck {
                "a stop code"
            } else {
                "the application name"
            };
            evidence.push(format!(
                "Windows did not record {missing} for these events."
            ));
            evidence.push("Timing alone does not link them to any other group or cause.".into());
            (
                "evidence-poor",
                format!("{} with missing details", kind.title()),
                format!(
                    "{} {times}, without enough recorded detail to group by cause.",
                    kind.title()
                ),
                "Check nearby records for details Windows didn't record here.".into(),
            )
        }
        Key::Other(kind) => {
            evidence.push(format!(
                "{} Event {} from {}.",
                kind.channel(),
                kind.demo_id(),
                kind.provider()
            ));
            (
                "signature",
                kind.title().into(),
                format!("{} {times}. {}", kind.title(), kind.explanation()),
                kind.next_step().into(),
            )
        }
    };
    let mut records: Vec<Record> = list
        .iter()
        .flat_map(|incident| {
            let pair = incident.events.len() > 1;
            incident
                .events
                .iter()
                .map(move |event| (pair, *event, incident))
        })
        .map(|(pair, event, incident)| {
            let partner = incident
                .events
                .iter()
                .find(|other| other.record != event.record);
            Record {
                at: event.at.to_rfc3339(),
                record: event.record.to_string(),
                id: event.id,
                channel: event.kind.channel(),
                provider: event.kind.provider(),
                title: event.kind.title(),
                explanation: event.kind.explanation(),
                next_step: event.kind.next_step(),
                help: event.kind.help(),
                application: event
                    .detail
                    .application
                    .as_deref()
                    .map(|a| names.application(a)),
                module: event.detail.module.as_deref().map(|m| names.module(m)),
                exception: event.detail.exception,
                stop_code: event.detail.stop_code,
                linked: pair.then(|| {
                    let partner = partner.expect("a pair has two events");
                    format!("{}-{}", partner.kind.channel(), partner.record)
                }),
            }
        })
        .collect();
    records.sort_by(|a, b| b.at.cmp(&a.at).then(b.record.cmp(&a.record)));
    let frequency = Frequency {
        days7: per_day(list, now, 7),
        days30: per_day(list, now, 30),
        days90: per_day(list, now, 90),
    };
    let mut group = Group {
        key: String::new(),
        basis,
        title,
        explanation,
        evidence,
        next_check,
        help: anchor.kind.help(),
        incidents: count,
        first: first.to_rfc3339(),
        last: last.to_rfc3339(),
        frequency,
        records,
        clues: Vec::new(),
    };
    group.clues = clue_lines(&group, last, clues, names.include);
    group
}

fn per_day(list: &[Incident], now: DateTime<Utc>, days: usize) -> Vec<usize> {
    let today = now.date_naive();
    let mut counts = vec![0; days];
    for incident in list {
        let age = (today - incident.at().date_naive()).num_days();
        if (0..days as i64).contains(&age) {
            counts[days - 1 - age as usize] += 1;
        }
    }
    counts
}

fn bracketed(group: &Group, before: &Snapshot, after: &Snapshot) -> bool {
    let time = |value: &str| {
        DateTime::parse_from_rfc3339(value)
            .map(|t| t.with_timezone(&Utc))
            .ok()
    };
    match (
        time(&before.time_utc),
        time(&after.time_utc),
        time(&group.first),
    ) {
        (Some(before), Some(after), Some(first)) => before <= first && first <= after,
        _ => false,
    }
}

fn clue_lines(
    group: &Group,
    last: DateTime<Utc>,
    clues: Clues,
    include_names: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(capture) = clues.health {
        let window = (last - TimeDelta::minutes(10), last + TimeDelta::minutes(10));
        if let Some((sample, gap)) = capture.nearest(last, window) {
            let readings: Vec<_> = METRICS
                .into_iter()
                .zip(sample.metrics())
                .filter_map(|((name, unit), value)| {
                    value.map(|value| format!("{name} {value:.1}{unit}"))
                })
                .collect();
            if !readings.is_empty() {
                let when = match gap {
                    0 => "at the time of".to_owned(),
                    gap if gap < 0 => format!("{:.1} s before", gap.unsigned_abs() as f64 / 1000.0),
                    gap => format!("{:.1} s after", gap as f64 / 1000.0),
                };
                lines.push(format!(
                    "Timing clue: the health recording {when} the latest incident read {}.",
                    readings.join(", ")
                ));
            }
        }
    }
    if let Some((before, after)) = clues.versions
        && bracketed(group, before, after)
    {
        for section in GROUPS {
            let Some(changes) = before.changes(after, section) else {
                continue;
            };
            if !include_names {
                if !changes.is_empty() {
                    lines.push(format!(
                        "Timing clue: {section} changed between the inventory snapshots."
                    ));
                }
                continue;
            }
            for change in changes.into_iter().take(4) {
                lines.push(format!(
                    "Timing clue: between the inventory snapshots, {section} · {} changed from {} to {}.",
                    change.label,
                    change.before.as_deref().unwrap_or("not installed"),
                    change.after.as_deref().unwrap_or("not installed"),
                ));
            }
        }
    }
    lines
}

/// The analysis day used for fictional history so screenshots and tests are stable.
pub fn demo_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-24T00:00:00Z")
        .expect("valid demo date")
        .with_timezone(&Utc)
}

/// Fictional 90-day crash history: a recurring crash, a similar crash with a
/// different signature, a linked stop-error pair, an ambiguous restart pair
/// and events with missing records.
pub fn demo_events() -> Vec<Event> {
    let now = demo_now();
    let crash = |application: Option<&str>, module: Option<&str>, exception: Option<u32>| Detail {
        application: application.map(Into::into),
        module: module.map(Into::into),
        exception,
        stop_code: None,
    };
    let stop = |code: Option<u32>| Detail {
        stop_code: code,
        ..Default::default()
    };
    let editor = || {
        crash(
            Some("demo-editor.exe"),
            Some("demo-renderer.dll"),
            Some(0xC000_0005),
        )
    };
    let entries: Vec<(i64, i64, Kind, Detail)> = vec![
        (40, 600, Kind::Crash, editor()),
        (30, 120, Kind::Bugcheck, stop(None)),
        (25, 300, Kind::Restart, Detail::default()),
        (
            20,
            900,
            Kind::Crash,
            crash(Some("demo-sync.exe"), None, Some(0xE043_4352)),
        ),
        (15, 540, Kind::Restart, Detail::default()),
        (15, 542, Kind::Restart, Detail::default()),
        (15, 543, Kind::Bugcheck, stop(Some(0x9F))),
        (12, 780, Kind::Crash, editor()),
        (9, 1260, Kind::GraphicsReset, Detail::default()),
        (8, 600, Kind::Restart, Detail::default()),
        (8, 601, Kind::Bugcheck, stop(Some(0x9F))),
        (
            6,
            660,
            Kind::Crash,
            crash(
                Some("demo-editor.exe"),
                Some("demo-spell.dll"),
                Some(0xC000_0409),
            ),
        ),
        (5, 420, Kind::Crash, crash(None, None, None)),
        (4, 1020, Kind::Crash, editor()),
        (3, 840, Kind::Crash, editor()),
        (3, 1200, Kind::GraphicsReset, Detail::default()),
        (
            2,
            480,
            Kind::Crash,
            crash(Some("demo-sync.exe"), None, Some(0xE043_4352)),
        ),
        (
            1,
            720,
            Kind::Hang,
            crash(Some("demo-editor.exe"), None, None),
        ),
        (1, 722, Kind::Crash, editor()),
    ];
    entries
        .into_iter()
        .enumerate()
        .map(|(index, (days, minute, kind, detail))| Event {
            at: now - TimeDelta::days(days) + TimeDelta::minutes(minute),
            record: 4100 + index as u64,
            kind,
            id: kind.demo_id(),
            detail,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group<'a>(analysis: &'a Analysis, title: &str) -> &'a Group {
        analysis
            .groups
            .iter()
            .find(|group| group.title == title)
            .unwrap_or_else(|| panic!("missing {title}: {:#?}", analysis.groups))
    }

    #[test]
    fn groups_recurring_crashes_by_signature() {
        let analysis = analyze(&demo_events(), demo_now(), true, Clues::default());
        let editor = group(&analysis, "demo-editor.exe crashed in demo-renderer.dll");
        assert_eq!(editor.incidents, 5);
        assert_eq!(editor.records.len(), 5);
        assert!(
            editor
                .evidence
                .iter()
                .any(|line| line.contains("1 other signature"))
        );
        assert_eq!(editor.frequency.days7.iter().sum::<usize>(), 3);
        assert_eq!(editor.frequency.days30.iter().sum::<usize>(), 4);
        assert_eq!(editor.frequency.days90.iter().sum::<usize>(), 5);
        assert_eq!(editor.frequency.days7.len(), 7);
        assert!(editor.first < editor.last);
        let spell = group(&analysis, "demo-editor.exe crashed in demo-spell.dll");
        assert_eq!(spell.incidents, 1);
        let sync = group(&analysis, "demo-sync.exe crashed");
        assert!(
            sync.evidence
                .iter()
                .any(|line| line.contains("did not record a faulting module"))
        );
    }

    #[test]
    fn links_only_clear_stop_error_pairs_and_keeps_both_records() {
        let analysis = analyze(&demo_events(), demo_now(), true, Clues::default());
        let stop = group(
            &analysis,
            "Windows stop error 0x0000009F (DRIVER_POWER_STATE_FAILURE)",
        );
        // One clear pair plus one stop error whose restart is ambiguous.
        assert_eq!(stop.incidents, 2);
        assert_eq!(stop.records.len(), 3);
        assert_eq!(
            stop.records
                .iter()
                .filter(|record| record.linked.is_some())
                .count(),
            2
        );
        let unexplained = group(&analysis, "Unexplained restart");
        assert_eq!(unexplained.basis, "unexplained");
        assert_eq!(unexplained.incidents, 3);
        assert!(
            unexplained
                .evidence
                .iter()
                .any(|line| line.contains("2 of these restarts"))
        );
        assert!(unexplained.explanation.contains("undetermined"));
        // Every incident event stays visible in exactly one group.
        let records: usize = analysis
            .groups
            .iter()
            .map(|group| group.records.len())
            .sum();
        assert_eq!(records, demo_events().len());
        assert_eq!(analysis.incidents, demo_events().len() - 1);
    }

    #[test]
    fn keeps_evidence_poor_events_separate_and_last() {
        let analysis = analyze(&demo_events(), demo_now(), true, Clues::default());
        let poor = group(&analysis, "Application crashed with missing details");
        assert_eq!(poor.basis, "evidence-poor");
        assert_eq!(
            group(&analysis, "Windows stop error with missing details").incidents,
            1
        );
        let first_poor = analysis
            .groups
            .iter()
            .position(|group| matches!(group.basis, "unexplained" | "evidence-poor"))
            .unwrap();
        assert!(
            analysis.groups[first_poor..]
                .iter()
                .all(|group| matches!(group.basis, "unexplained" | "evidence-poor"))
        );
        assert!(
            analysis
                .groups
                .iter()
                .all(|group| !group.next_check.is_empty())
        );
    }

    #[test]
    fn redaction_hides_names_but_keeps_the_same_groups() {
        let named = analyze(&demo_events(), demo_now(), true, Clues::default());
        let hidden = analyze(&demo_events(), demo_now(), false, Clues::default());
        assert_eq!(named.incidents, hidden.incidents);
        assert_eq!(
            named.groups.iter().map(|g| g.incidents).collect::<Vec<_>>(),
            hidden
                .groups
                .iter()
                .map(|g| g.incidents)
                .collect::<Vec<_>>()
        );
        let text = serde_json::to_string(&hidden).unwrap();
        assert!(!text.contains("demo-editor") && !text.contains("demo-renderer"));
        assert!(text.contains("Application A") && text.contains("Module A"));
    }

    #[test]
    fn shows_changes_only_from_snapshots_around_the_incident() {
        let (before, after) = Snapshot::demo();
        let events = vec![Event {
            at: DateTime::parse_from_rfc3339(&before.time_utc)
                .unwrap()
                .with_timezone(&Utc)
                + TimeDelta::minutes(1),
            record: 1,
            kind: Kind::GraphicsReset,
            id: 4101,
            detail: Detail::default(),
        }];
        let clues = Clues {
            health: None,
            versions: Some((&before, &after)),
        };
        let analysis = analyze(&events, demo_now(), true, clues);
        assert!(
            analysis.groups[0]
                .clues
                .iter()
                .any(|line| line.starts_with("Timing clue"))
        );
        let late = analyze(&demo_events()[..1], demo_now(), true, clues);
        assert!(late.groups[0].clues.is_empty());
        assert!(
            late.notices
                .iter()
                .any(|line| line.contains("not taken on either side"))
        );
    }

    #[test]
    fn redacts_inventory_clue_names_and_versions_in_desktop_data_and_html() {
        use crate::report::{Session, render_history};

        let now = demo_now();
        let snapshot = |time: DateTime<Utc>, version: &str| {
            serde_json::from_value::<Snapshot>(serde_json::json!({
                "schema": 1,
                "time_utc": time.to_rfc3339(),
                "items": {
                    "Installed desktop software": {
                        "fictional-private-editor": {
                            "label": "Fictional Private Editor",
                            "versions": [version]
                        }
                    }
                }
            }))
            .unwrap()
        };
        let before = snapshot(now - TimeDelta::days(90), "fictional-old-version");
        let after = snapshot(now, "fictional-new-version");
        let session = Session {
            window: None,
            events: demo_events(),
            notices: vec![],
        };
        for include_names in [false, true] {
            let analysis = analyze(
                &session.events,
                now,
                include_names,
                Clues {
                    health: None,
                    versions: Some((&before, &after)),
                },
            );
            let desktop = serde_json::to_string(&analysis).unwrap();
            let html = render_history("Fictional", &session, &analysis, include_names);
            for output in [&desktop, &html] {
                assert!(output.contains("Installed desktop software"));
                for private in [
                    "Fictional Private Editor",
                    "fictional-old-version",
                    "fictional-new-version",
                ] {
                    assert_eq!(output.contains(private), include_names);
                }
            }
        }
    }

    #[test]
    fn handles_missing_records() {
        let analysis = analyze(&[], demo_now(), false, Clues::default());
        assert_eq!(analysis.incidents, 0);
        assert!(analysis.groups.is_empty());
        assert_eq!(label("Application", 27), "Application AB");
    }
}
