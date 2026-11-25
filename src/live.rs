use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, TimeDelta, Utc};

use crate::incidents::Event;
use crate::saved::{Bookmark, Channel};

pub struct Batch {
    pub lines: Vec<Line>,
    pub notices: Vec<String>,
    pub health: Option<crate::capture::Sample>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub reference: Option<Bookmark>,
}

pub fn watch(mut take: impl FnMut(Batch) -> bool) -> Result<(), String> {
    let mut seen = HashMap::new();
    let mut first = true;
    loop {
        let now = DateTime::<Utc>::from(SystemTime::now());
        let start = now
            - if first {
                TimeDelta::minutes(10)
            } else {
                TimeDelta::seconds(90)
            };
        let session = crate::collect(None, Some((start, now)))?;
        if !take(Batch {
            lines: new_lines(&mut seen, &session.events, start),
            notices: session.notices,
            health: None,
        }) {
            return Ok(());
        }
        first = false;
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn line(event: &Event) -> Line {
    let detail = event
        .detail
        .application
        .as_ref()
        .map_or(String::new(), |name| format!(" · {name}"));
    let text = format!(
        "{}  {}{detail}  [{} · ID {} · record {}]",
        event.at.format("%Y-%m-%d %H:%M:%S UTC"),
        event.kind.title(),
        event.kind.channel(),
        event.id,
        event.record,
    );
    Line {
        text,
        reference: Some(Bookmark {
            channel: if event.kind.channel() == "System" {
                Channel::System
            } else {
                Channel::Application
            },
            record: event.record,
            time_utc: event.at.to_rfc3339(),
            title: event.kind.title().into(),
        }),
    }
}

type Seen = HashMap<(&'static str, u64), DateTime<Utc>>;

fn new_lines(seen: &mut Seen, events: &[Event], start: DateTime<Utc>) -> Vec<Line> {
    seen.retain(|_, at| *at >= start);
    events
        .iter()
        .filter_map(|event| {
            let prior = seen.insert((event.kind.channel(), event.record), event.at);
            (prior != Some(event.at)).then(|| line(event))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_polls_do_not_show_duplicate_records() {
        let events = crate::incidents::demo();
        let mut seen = Seen::new();
        let start = events[0].at - TimeDelta::seconds(1);
        assert_eq!(new_lines(&mut seen, &events, start).len(), events.len());
        assert!(new_lines(&mut seen, &events, start).is_empty());
        let mut next = events[0].clone();
        next.at += TimeDelta::minutes(2);
        let later = new_lines(&mut seen, &[next], start);
        assert_eq!(later.len(), 1);
        let marker = later[0].reference.as_ref().unwrap();
        assert_eq!((marker.channel, marker.record), (Channel::System, 1));
    }
}
