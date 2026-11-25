use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const GROUPS: [&str; 5] = [
    "Windows",
    "Firmware",
    "Display adapters",
    "Storage",
    "Installed desktop software",
];

type Items = BTreeMap<String, Item>;

#[derive(Deserialize, Serialize)]
pub struct Snapshot {
    schema: u8,
    pub time_utc: String,
    items: BTreeMap<String, Option<Items>>,
}

#[derive(Deserialize, Serialize)]
struct Item {
    label: String,
    versions: BTreeSet<String>,
}

pub struct Difference {
    pub label: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Snapshot {
    #[cfg(windows)]
    pub fn from_windows(data: &str) -> Result<Self, String> {
        let data: Value = serde_json::from_str(data)
            .map_err(|error| format!("Windows returned invalid version data. {error}"))?;
        if !data.is_object() {
            return Err("Windows did not return version data.".into());
        }
        let time = DateTime::<Utc>::from(std::time::SystemTime::now());
        Ok(Self::from_sections(&data, time))
    }

    fn from_sections(data: &Value, time: DateTime<Utc>) -> Self {
        let items = GROUPS
            .into_iter()
            .map(|group| {
                let items = data[group].as_array().and_then(|rows| {
                    if rows.is_empty() && matches!(group, "Windows" | "Firmware") {
                        return None;
                    }
                    let mut items = Items::new();
                    for row in rows {
                        if let Some((label, version)) = version_from_row(group, row) {
                            items
                                .entry(label.to_lowercase())
                                .or_insert_with(|| Item {
                                    label,
                                    versions: BTreeSet::new(),
                                })
                                .versions
                                .insert(version.unwrap_or_else(|| "Version unavailable".into()));
                        }
                    }
                    Some(items)
                });
                (group.into(), items)
            })
            .collect();
        Self {
            schema: 1,
            time_utc: time.to_rfc3339(),
            items,
        }
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| format!("Could not open the version snapshot. {error}"))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("The version snapshot is too large.".into());
        }
        let snapshot: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("This file is not a Parsley version snapshot. {error}"))?;
        if snapshot.schema != 1 || DateTime::parse_from_rfc3339(&snapshot.time_utc).is_err() {
            return Err("This version snapshot uses an unsupported format.".into());
        }
        Ok(snapshot)
    }

    #[cfg(windows)]
    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    pub fn changes(&self, after: &Self, group: &str) -> Option<Vec<Difference>> {
        let before = self.items.get(group)?.as_ref()?;
        let after = after.items.get(group)?.as_ref()?;
        let keys: BTreeSet<_> = before.keys().chain(after.keys()).collect();
        Some(
            keys.into_iter()
                .filter_map(|key| {
                    let old = before.get(key);
                    let new = after.get(key);
                    if old.map(|item| &item.versions) == new.map(|item| &item.versions) {
                        return None;
                    }
                    let join =
                        |item: &Item| item.versions.iter().cloned().collect::<Vec<_>>().join(", ");
                    Some(Difference {
                        label: new.or(old)?.label.clone(),
                        before: old.map(join),
                        after: new.map(join),
                    })
                })
                .collect(),
        )
    }

    pub fn demo() -> (Self, Self) {
        let time = DateTime::parse_from_rfc3339("2026-11-21T06:14:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let data = |os, driver, editor| {
            serde_json::json!({
                "Windows": [{"Caption": "Fictional Windows 11", "Version": "10.0", "BuildNumber": os}],
                "Firmware": [{"Manufacturer":"Demo System","SMBIOSBIOSVersion":"1.0"}],
                "Display adapters": [{"Name": "Demo Graphics", "DriverVersion": driver}],
                "Storage": [{"Model": "Demo Drive", "FirmwareRevision": "2.4"}],
                "Installed desktop software": [{"DisplayName": "Demo Editor", "Publisher": "Demo Lab", "DisplayVersion": editor}]
            })
        };
        (
            Self::from_sections(&data("26100", "1.3", "2.0"), time),
            Self::from_sections(
                &data("26200", "1.4", "2.0"),
                time + chrono::TimeDelta::days(1),
            ),
        )
    }
}

fn version_from_row(group: &str, row: &Value) -> Option<(String, Option<String>)> {
    let text = |name: &str| {
        row[name]
            .as_str()
            .map(|value| {
                value
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(240)
                    .collect::<String>()
            })
            .filter(|value| !value.is_empty())
    };
    match group {
        "Windows" => {
            let version = [
                text("Caption"),
                text("Version"),
                text("BuildNumber").map(|build| format!("build {build}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            Some(("Windows".into(), (!version.is_empty()).then_some(version)))
        }
        "Firmware" => Some((text("Manufacturer")?, text("SMBIOSBIOSVersion"))),
        "Display adapters" => Some((text("Name")?, text("DriverVersion"))),
        "Storage" => Some((text("Model")?, text("FirmwareRevision"))),
        "Installed desktop software" => {
            let name = text("DisplayName")?;
            let label =
                text("Publisher").map_or(name.clone(), |publisher| format!("{name} · {publisher}"));
            Some((label, text("DisplayVersion")))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_diff_does_not_show_unchanged_versions_or_missing_groups_as_removals() {
        let (before, mut after) = Snapshot::demo();
        assert!(before.changes(&after, "Firmware").unwrap().is_empty());
        let change = &before.changes(&after, "Display adapters").unwrap()[0];
        assert_eq!(change.before.as_deref(), Some("1.3"));
        assert_eq!(change.after.as_deref(), Some("1.4"));
        after.items.insert("Display adapters".into(), None);
        assert!(before.changes(&after, "Display adapters").is_none());
    }
}
