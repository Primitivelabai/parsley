use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Channel {
    Application,
    System,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Application => "Application",
            Self::System => "System",
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Saved {
    pub searches: Vec<String>,
    pub bookmarks: Vec<Bookmark>,
    pub last_recording: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Bookmark {
    pub channel: Channel,
    pub record: u64,
    pub time_utc: String,
    pub title: String,
}

impl Bookmark {
    pub fn caption(&self) -> String {
        format!(
            "{}  {}  [{} record {}]",
            self.time_utc,
            self.title,
            self.channel.name(),
            self.record
        )
    }

    fn same_record(&self, other: &Self) -> bool {
        self.channel == other.channel
            && self.record == other.record
            && self.time_utc == other.time_utc
    }
}

pub struct Store(PathBuf);

impl Store {
    pub fn for_user() -> Result<Self, String> {
        let path = std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .ok_or("Windows did not provide a local app data folder.")?;
        Ok(Self(PathBuf::from(path).join("Parsley").join("saved.json")))
    }

    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn read(&self) -> Result<Saved, String> {
        let file = match File::open(&self.0) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Saved::default()),
            Err(error) => return Err(format!("Could not read saved Parsley items. {error}")),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("The saved Parsley file is too large.".into());
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("The saved Parsley file could not be read. {error}"))
    }

    pub fn change(
        &self,
        update: impl FnOnce(&mut Saved) -> Result<(), String>,
    ) -> Result<Saved, String> {
        let folder = self
            .0
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(folder)
            .map_err(|error| format!("Could not open local Parsley storage. {error}"))?;
        // Share mode zero makes the read-modify-write exclusive across running instances.
        let _owner = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(self.0.with_extension("lock"))
            .map_err(|error| format!("Could not lock saved Parsley items. {error}"))?;
        let mut saved = self.read()?;
        update(&mut saved)?;
        let bytes = serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("The saved Parsley file is full. Remove some older items first.".into());
        }
        let mut draft =
            tempfile::NamedTempFile::new_in(folder).map_err(|error| error.to_string())?;
        draft.write_all(&bytes).map_err(|error| error.to_string())?;
        draft
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        draft
            .persist(&self.0)
            .map_err(|error| format!("Could not save Parsley items. {}", error.error))?;
        Ok(saved)
    }
}

impl Saved {
    pub fn add_search(&mut self, input: &str) -> Result<(), String> {
        let input = input.trim();
        if input.is_empty() || input.len() > 512 || input.chars().any(char::is_control) {
            return Err("Enter a non-empty search of up to 512 bytes.".into());
        }
        if !self
            .searches
            .iter()
            .any(|query| query.to_lowercase() == input.to_lowercase())
        {
            self.searches.push(input.into());
        }
        Ok(())
    }

    pub fn bookmark(&mut self, marker: Bookmark) {
        if !self
            .bookmarks
            .iter()
            .any(|saved| saved.same_record(&marker))
        {
            self.bookmarks.push(marker);
        }
    }

    pub fn remove_bookmark(&mut self, marker: &Bookmark) {
        self.bookmarks.retain(|saved| !saved.same_record(marker));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_items_survive_restart_without_duplicate_or_recycled_ids() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("saved.json");
        let store = Store::new(path.clone());
        let bookmark = Bookmark {
            channel: Channel::System,
            record: 42,
            time_utc: "2026-11-22T06:00:00+00:00".into(),
            title: "Unexpected restart".into(),
        };
        store
            .change(|saved| {
                saved.add_search("Driver")?;
                saved.add_search(" driver ")?;
                saved.bookmark(bookmark.clone());
                saved.bookmark(bookmark.clone());
                let mut recycled = bookmark.clone();
                recycled.time_utc = "2026-11-23T06:00:00+00:00".into();
                saved.bookmark(recycled);
                Ok(())
            })
            .unwrap();
        let reloaded = Store::new(path);
        let read = reloaded.read().unwrap();
        assert_eq!(read.searches, ["Driver"]);
        assert_eq!(read.bookmarks.len(), 2);
        let removed = reloaded
            .change(|saved| {
                saved.remove_bookmark(&bookmark);
                Ok(())
            })
            .unwrap();
        assert_eq!(removed.bookmarks.len(), 1);
        assert_ne!(removed.bookmarks[0].time_utc, bookmark.time_utc);
    }
}
