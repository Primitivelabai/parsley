use std::fs::File;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::report::Window;

pub const MAX_SAMPLES: usize = 3600;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const RECORDING_KIND: &str = "parsley-health-recording";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sample {
    pub unix_ms: i64,
    pub cpu_percent: Option<f64>,
    pub ram_percent: Option<f64>,
    pub gpu_peak_percent: Option<f64>,
    pub disk_mib_per_sec: Option<f64>,
    pub network_kib_per_sec: Option<f64>,
}

#[derive(Deserialize, Serialize)]
pub struct Capture {
    schema: u8,
    samples: Vec<Sample>,
}

#[derive(Deserialize, Serialize)]
pub struct JournalHeader {
    kind: String,
    schema: u8,
    pub id: String,
    pub minutes: u32,
    pub started_utc: String,
}

impl JournalHeader {
    #[cfg(windows)]
    pub fn new(id: String, minutes: u32, started: DateTime<Utc>) -> Self {
        Self {
            kind: RECORDING_KIND.into(),
            schema: 1,
            id,
            minutes,
            started_utc: started.to_rfc3339(),
        }
    }

    pub fn valid(&self) -> bool {
        self.kind == RECORDING_KIND
            && self.schema == 1
            && valid_id(&self.id)
            && (1..=60).contains(&self.minutes)
            && DateTime::parse_from_rfc3339(&self.started_utc).is_ok()
    }
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEntry {
    Sample { value: Sample },
    End { reason: EndReason },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Finished,
    Stopped,
    Failed,
}

pub struct Journal {
    #[cfg(windows)]
    pub header: JournalHeader,
    pub capture: Capture,
    #[cfg(windows)]
    pub completed: Option<EndReason>,
}

impl Journal {
    #[cfg(windows)]
    pub fn from_file(file: &File) -> Result<Self, String> {
        Self::parse(&read_capture_bytes(file)?)
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        // A live reader may see the writer between bytes of its last line.
        // Only newline-terminated entries are visible as part of a capture.
        let end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .ok_or("The recording header is incomplete.")?;
        let mut lines = bytes[..end].split(|byte| *byte == b'\n');
        let header: JournalHeader =
            serde_json::from_slice(lines.next().ok_or("The recording header is missing.")?)
                .map_err(|_| "This file is not a Parsley background health recording.")?;
        if !header.valid() {
            return Err("This Parsley recording has an unsupported header.".into());
        }
        let mut samples = Vec::new();
        let mut completed = None;
        for line in lines {
            if completed.is_some() {
                return Err("The recording has entries after it ended.".into());
            }
            let entry = serde_json::from_slice(line)
                .map_err(|_| "The recording contains an invalid entry.")?;
            match entry {
                JournalEntry::Sample { value } => {
                    if !value.valid() || samples.len() >= MAX_SAMPLES {
                        return Err(
                            "The recording contains invalid data or too many samples.".into()
                        );
                    }
                    samples.push(value);
                }
                JournalEntry::End { reason } => completed = Some(reason),
            }
        }
        Ok(Self {
            #[cfg(windows)]
            header,
            capture: Capture::from_samples(samples),
            #[cfg(windows)]
            completed,
        })
    }
}

#[derive(Default)]
pub struct Summary {
    pub samples: usize,
    pub coverage: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub peak: [Option<f64>; 5],
}

pub const METRICS: [(&str, &str); 5] = [
    ("CPU", "%"),
    ("Physical memory", "%"),
    ("Busiest GPU engine", "%"),
    ("Disk read and write", " MiB/s"),
    ("Physical network received and sent", " KiB/s"),
];

impl Sample {
    pub fn metrics(&self) -> [Option<f64>; 5] {
        [
            self.cpu_percent,
            self.ram_percent,
            self.gpu_peak_percent,
            self.disk_mib_per_sec,
            self.network_kib_per_sec,
        ]
    }

    fn valid(&self) -> bool {
        DateTime::from_timestamp_millis(self.unix_ms).is_some()
            && !self.metrics().iter().enumerate().any(|(index, value)| {
                value.is_some_and(|value| {
                    !value.is_finite() || value < 0.0 || (index < 3 && value > 100.0)
                })
            })
    }
}

impl Capture {
    pub fn from_samples(mut samples: Vec<Sample>) -> Self {
        samples.sort_by_key(|sample| sample.unix_ms);
        Self { schema: 1, samples }
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    pub fn nearest(&self, at: DateTime<Utc>, (start, end): Window) -> Option<(&Sample, i64)> {
        let at = at.timestamp_millis();
        let start = start.timestamp_millis();
        let end = end.timestamp_millis();
        let index = self.samples.partition_point(|sample| sample.unix_ms < at);
        let before = index
            .checked_sub(1)
            .and_then(|index| self.samples.get(index));
        let after = self.samples.get(index);
        [before, after]
            .into_iter()
            .flatten()
            .filter(|sample| {
                (start..=end).contains(&sample.unix_ms) && sample.unix_ms.abs_diff(at) <= 10_000
            })
            .min_by_key(|sample| (sample.unix_ms.abs_diff(at), sample.unix_ms))
            .map(|sample| (sample, sample.unix_ms - at))
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = read_bytes(path)?;
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        {
            return Journal::parse(&bytes).map(|journal| journal.capture);
        }
        let capture: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("This is not a valid Parsley health capture. {error}"))?;
        if capture.schema != 1 || capture.samples.len() > MAX_SAMPLES {
            return Err("The health capture version or sample count is unsupported.".into());
        }
        if capture.samples.iter().any(|sample| !sample.valid()) {
            return Err("The health capture contains an invalid timestamp or measurement.".into());
        }
        Ok(Self::from_samples(capture.samples))
    }

    pub fn summarize(&self, (start, end): Window) -> Summary {
        let start = self
            .samples
            .partition_point(|sample| sample.unix_ms < start.timestamp_millis());
        let end = self
            .samples
            .partition_point(|sample| sample.unix_ms <= end.timestamp_millis());
        let samples = &self.samples[start..end];
        let mut peak = [None::<f64>; 5];
        for sample in samples {
            for (maximum, value) in peak.iter_mut().zip(sample.metrics()) {
                if let Some(value) = value {
                    *maximum = Some(maximum.map_or(value, |current| current.max(value)));
                }
            }
        }
        let coverage = samples
            .first()
            .zip(samples.last())
            .and_then(|(first, last)| {
                DateTime::from_timestamp_millis(first.unix_ms)
                    .zip(DateTime::from_timestamp_millis(last.unix_ms))
            });
        Summary {
            samples: samples.len(),
            coverage,
            peak,
        }
    }

    pub fn demo(incident: DateTime<Utc>, baseline: DateTime<Utc>) -> Self {
        let mut samples = Vec::new();
        for (centre, high_load) in [(incident, true), (baseline, false)] {
            for offset in -3..=3 {
                let peak = high_load && offset == 0;
                samples.push(Sample {
                    unix_ms: (centre + TimeDelta::minutes(offset)).timestamp_millis(),
                    cpu_percent: Some(if peak { 93.0 } else { 18.0 + offset as f64 }),
                    ram_percent: Some(if peak { 83.0 } else { 52.0 }),
                    gpu_peak_percent: Some(if peak { 97.0 } else { 21.0 }),
                    disk_mib_per_sec: Some(if peak { 11.0 } else { 1.5 }),
                    network_kib_per_sec: None,
                });
            }
        }
        Self::from_samples(samples)
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path)
        .map_err(|error| format!("Could not read the Parsley health capture. {error}"))?;
    read_capture_bytes(&file)
}

fn read_capture_bytes(file: &File) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read the Parsley health capture. {error}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("The Parsley health capture is too large.".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_samples_inside_the_window_count_and_missing_metrics_stay_unknown() {
        let incident = DateTime::from_timestamp(1000, 0).unwrap();
        let baseline = incident + TimeDelta::days(1);
        let capture = Capture::demo(incident, baseline);
        let radius = TimeDelta::minutes(1);
        let bad = capture.summarize((incident - radius, incident + radius));
        let good = capture.summarize((baseline - radius, baseline + radius));
        assert_eq!(bad.samples, 3);
        assert_eq!(good.samples, 3);
        assert_eq!(
            bad.peak,
            [Some(93.0), Some(83.0), Some(97.0), Some(11.0), None]
        );
        assert_eq!(good.peak[0], Some(19.0));
        assert!(
            capture
                .summarize((
                    incident + TimeDelta::hours(1),
                    baseline - TimeDelta::hours(1)
                ))
                .coverage
                .is_none()
        );
    }

    #[test]
    fn a_live_recording_ignores_only_an_unfinished_trailing_entry() {
        let start = DateTime::from_timestamp(1000, 0).unwrap();
        let header = JournalHeader {
            kind: RECORDING_KIND.into(),
            schema: 1,
            id: "12345678901234567890123456789012".into(),
            minutes: 5,
            started_utc: start.to_rfc3339(),
        };
        let mut input = serde_json::to_string(&header).unwrap();
        input.push('\n');
        let sample = Capture::demo(start, start + TimeDelta::days(1))
            .samples
            .remove(0);
        input.push_str(&serde_json::to_string(&JournalEntry::Sample { value: sample }).unwrap());
        input.push('\n');
        let complete_lines = input.clone();
        input.push_str("{\"kind\":\"sample\"");
        let journal = Journal::parse(input.as_bytes()).unwrap();
        assert_eq!(journal.capture.sample_count(), 1);
        input.push('\n');
        assert!(Journal::parse(input.as_bytes()).is_err());
        let footer = serde_json::to_string(&JournalEntry::End {
            reason: EndReason::Stopped,
        })
        .unwrap();
        let valid_end = format!("{complete_lines}{footer}\n");
        assert!(Journal::parse(valid_end.as_bytes()).is_ok());
        assert!(Journal::parse(format!("{valid_end}{footer}\n").as_bytes()).is_err());
    }

    #[test]
    fn nearby_samples_respect_time_window_and_maximum_gap() {
        let at = DateTime::from_timestamp(1000, 0).unwrap();
        let capture = Capture::demo(at, at + TimeDelta::days(1));
        let range = (at - TimeDelta::minutes(1), at + TimeDelta::minutes(1));
        assert_eq!(
            capture
                .nearest(at + TimeDelta::seconds(2), range)
                .unwrap()
                .1,
            -2000
        );
        assert!(
            capture
                .nearest(at + TimeDelta::seconds(15), range)
                .is_none()
        );
        assert!(
            capture
                .nearest(at, (at + TimeDelta::seconds(1), range.1))
                .is_none()
        );
    }
}
