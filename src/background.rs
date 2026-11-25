use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use serde::Serialize;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject,
};

use crate::capture::{EndReason, Journal, JournalEntry, JournalHeader, MAX_SAMPLES, valid_id};

const EVENT_MODIFY_STATE: u32 = 0x0002;
const SYNCHRONIZE: u32 = 0x0010_0000;
const FILE_SHARE_READ: u32 = 0x1;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WORKER: &str = "--parsley-health-worker";

pub fn action(command: &str, mut args: impl Iterator<Item = OsString>) -> Result<String, String> {
    let path = PathBuf::from(args.next().ok_or("Choose a .jsonl recording file.")?);
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
    {
        return Err("Background recordings use .jsonl files.".into());
    }
    match command {
        "start-recording" => {
            let minutes = match args.next() {
                None => 15,
                Some(flag) if flag == "--minutes" => parse_minutes(args.next())?,
                _ => return Err("Use start-recording FILE.jsonl [--minutes N].".into()),
            };
            if args.next().is_some() {
                return Err("Use start-recording FILE.jsonl [--minutes N].".into());
            }
            start(&path, minutes)
        }
        "recording-status" | "stop-recording" => {
            if args.next().is_some() {
                return Err("Choose one .jsonl recording file.".into());
            }
            let stop = command == "stop-recording";
            status(&path, stop)
        }
        _ => Err("Unknown recording action.".into()),
    }
}

pub fn maybe_worker() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != WORKER {
        return None;
    }
    let mut run = || {
        let path = PathBuf::from(args.next().ok_or("Missing recording path.")?);
        let minutes = parse_minutes(args.next())?;
        let id = args
            .next()
            .and_then(|id| id.into_string().ok())
            .filter(|id| valid_id(id))
            .ok_or("Missing or invalid recording ID.")?;
        if args.next().is_some()
            || !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
        {
            return Err("Invalid background recording arguments.".into());
        }
        record(&path, minutes, id)
    };
    Some(match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Parsley: {error}");
            ExitCode::FAILURE
        }
    })
}

fn parse_minutes(value: Option<OsString>) -> Result<u32, String> {
    value
        .and_then(|value| value.to_str()?.parse().ok())
        .filter(|value| (1..=60).contains(value))
        .ok_or_else(|| "Choose a recording duration from 1 to 60 minutes.".into())
}

fn start(path: &Path, minutes: u32) -> Result<String, String> {
    let path = std::path::absolute(path).map_err(|error| error.to_string())?;
    if path.try_exists().map_err(|error| error.to_string())? {
        return Err("Choose a new recording file. Existing files are kept.".into());
    }
    if !path.parent().is_some_and(Path::is_dir) {
        return Err("The recording folder could not be found.".into());
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| "Could not allocate a unique recording ID.")?;
    let mut id = String::with_capacity(32);
    for byte in random {
        let _ = write!(id, "{byte:02x}");
    }
    let exe =
        std::env::current_exe().map_err(|error| format!("Could not locate Parsley. {error}"))?;
    let mut child = Command::new(exe)
        .arg(WORKER)
        .arg(&path)
        .arg(minutes.to_string())
        .arg(&id)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not launch the recorder. {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok((journal, file)) = read_journal(&path) {
            if journal.header.id != id {
                return Err("Another recording already created that file.".into());
            }
            if journal.completed.is_none() && StopEvent::open(&id, file, 0)?.is_some() {
                return Ok(format!(
                    "Recording for up to {minutes} minutes. You can close the Parsley window.\n{}",
                    path.display()
                ));
            }
        }
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("The background recorder ended before it could start. Check the selected recording path.".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    // This is the specific child just launched here, not a process found by PID.
    let _ = child.kill();
    let _ = child.wait();
    Err("The background recorder did not start in time.".into())
}

fn status(path: &Path, stop: bool) -> Result<String, String> {
    let (journal, file) = read_journal(path)?;
    if journal.completed.is_none() {
        let access = if stop { EVENT_MODIFY_STATE } else { 0 };
        if let Some(event) = StopEvent::open(&journal.header.id, file, access)? {
            if stop {
                event.stop()?;
                return Ok("Stop requested. The recorder is finishing the capture.".into());
            }
            return Ok(format!(
                "Recording is running · {} samples saved · up to {} minutes total.",
                journal.capture.sample_count(),
                journal.header.minutes
            ));
        }
        // The writer may have written the footer since the first read.
        return describe_ended(read_journal(path)?.0, stop);
    }
    describe_ended(journal, stop)
}

fn read_journal(path: &Path) -> Result<(Journal, FileIdentity), String> {
    let file =
        File::open(path).map_err(|error| format!("Could not open the recording. {error}"))?;
    let journal = Journal::from_file(&file)?;
    Ok((journal, FileIdentity::from_file(&file)?))
}

fn describe_ended(journal: Journal, stop: bool) -> Result<String, String> {
    let state = match journal.completed {
        Some(EndReason::Finished) => "Recording completed",
        Some(EndReason::Stopped) => "Recording stopped",
        Some(EndReason::Failed) => "Recording ended with a sampling error",
        None => "Recorder is inactive and the recording has no completion marker",
    };
    if stop && journal.completed.is_none() {
        return Err(format!(
            "{state}. The completed samples are still readable."
        ));
    }
    Ok(format!(
        "{state} · {} samples available.",
        journal.capture.sample_count()
    ))
}

fn record(path: &Path, minutes: u32, id: String) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|error| format!("Could not create the recording. {error}"))?;
    let event = StopEvent::create(&id, FileIdentity::from_file(&file)?)?;
    let mut output = BufWriter::new(file);
    let started = DateTime::<Utc>::from(SystemTime::now());
    write_line(&mut output, &JournalHeader::new(id, minutes, started))?;
    let deadline = Instant::now() + Duration::from_secs(u64::from(minutes) * 60);
    let mut count: usize = 0;
    let mut stopped = false;
    let mut failure = None;
    let mut write_failed = false;
    let sampling = crate::system::watch_health(false, |batch| {
        match event.stopped() {
            Ok(true) => {
                stopped = true;
                return false;
            }
            Err(error) => {
                failure = Some(error);
                return false;
            }
            Ok(false) => {}
        }
        if Instant::now() >= deadline || count >= MAX_SAMPLES {
            return false;
        }
        if let Some(value) = batch.health {
            count += 1;
            if let Err(error) = write_line(&mut output, &JournalEntry::Sample { value }) {
                failure = Some(error);
                write_failed = true;
                return false;
            }
            if count.is_multiple_of(5)
                && let Err(error) = output.get_ref().sync_data()
            {
                failure = Some(format!("Could not flush the health recording. {error}"));
                write_failed = true;
                return false;
            }
        }
        true
    });
    let failure = failure.or_else(|| sampling.err());
    if write_failed {
        // Do not append a footer after a potentially partial write.
        return Err(failure.unwrap_or_else(|| "The health recording could not be written.".into()));
    }
    let reason = if failure.is_some() {
        EndReason::Failed
    } else if stopped {
        EndReason::Stopped
    } else {
        EndReason::Finished
    };
    write_line(&mut output, &JournalEntry::End { reason })?;
    output
        .get_ref()
        .sync_data()
        .map_err(|error| format!("Could not finish saving the recording. {error}"))?;
    failure.map_or(Ok(()), Err)
}

fn write_line(output: &mut BufWriter<File>, value: &impl Serialize) -> Result<(), String> {
    let mut line = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    line.push(b'\n');
    output
        .write_all(&line)
        .and_then(|()| output.flush())
        .map_err(|error| format!("Could not write a health recording entry. {error}"))
}

struct StopEvent(HANDLE);

impl StopEvent {
    fn name(id: &str, file: FileIdentity) -> Vec<u16> {
        format!(
            "Local\\ParsleyHealth-{id}-{:08x}-{:08x}{:08x}",
            file.volume, file.high, file.low
        )
        .encode_utf16()
        .chain([0])
        .collect()
    }

    fn create(id: &str, file: FileIdentity) -> Result<Self, String> {
        let name = Self::name(id, file);
        // A session-local event uses the caller's security descriptor and is not inherited.
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "Could not initialise recorder controls. {}",
                io::Error::last_os_error()
            ));
        }
        let event = Self(handle);
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return Err("The recorder ID was already in use.".into());
        }
        Ok(event)
    }

    fn open(id: &str, file: FileIdentity, access: u32) -> Result<Option<Self>, String> {
        let name = Self::name(id, file);
        let access = if access == 0 { SYNCHRONIZE } else { access };
        let handle = unsafe { OpenEventW(access, 0, name.as_ptr()) };
        if !handle.is_null() {
            return Ok(Some(Self(handle)));
        }
        let error = unsafe { GetLastError() };
        if error == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        Err(format!(
            "Could not access recorder controls. {}",
            io::Error::from_raw_os_error(error as i32)
        ))
    }

    fn stopped(&self) -> Result<bool, String> {
        match unsafe { WaitForSingleObject(self.0, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            windows_sys::Win32::Foundation::WAIT_TIMEOUT => Ok(false),
            _ => Err(format!(
                "Could not check recorder controls. {}",
                io::Error::last_os_error()
            )),
        }
    }

    fn stop(&self) -> Result<(), String> {
        if unsafe { SetEvent(self.0) } == 0 {
            return Err(format!(
                "Could not send the stop request. {}",
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct FileIdentity {
    volume: u32,
    high: u32,
    low: u32,
}

impl FileIdentity {
    fn from_file(file: &File) -> Result<Self, String> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // Taking the ID from the open file handle binds controls to that file instance.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(format!(
                "Could not identify the recording file. {}",
                io::Error::last_os_error()
            ));
        }
        Ok(Self {
            volume: info.dwVolumeSerialNumber,
            high: info.nFileIndexHigh,
            low: info.nFileIndexLow,
        })
    }
}

impl Drop for StopEvent {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
