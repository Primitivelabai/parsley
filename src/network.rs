use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Write as _};
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle},
};
use std::process::ExitCode;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_CANCELLED, ERROR_NO_DATA,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, FILETIME, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Diagnostics::Etw::*;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_NOWAIT,
    PIPE_REJECT_REMOTE_CLIENTS,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    ReleaseMutex, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NO_CONSOLE, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows_sys::core::GUID;
use windows_sys::w;

const WORKER: &str = "--parsley-network-worker";
const SECONDS: u64 = 15;
const MAX_ROWS: usize = 4096;
const MAX_REPLY: usize = 2_000_000;
const EVENT_IDS: [u16; 8] = [10, 11, 26, 27, 42, 43, 58, 59];
const PROVIDER: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88);
const SESSION_ID: GUID = GUID::from_u128(0x4fd289a7_85ae_4cd5_a1ef_25cf3b5d6cdc);
const SESSION: *const u16 = w!("Primitive Parsley Bandwidth");

#[derive(Default, Serialize, Deserialize)]
struct Bytes {
    sent: u64,
    received: u64,
}

impl Bytes {
    fn total(&self) -> u64 {
        self.sent.saturating_add(self.received)
    }
}

#[derive(Serialize, Deserialize)]
struct Row {
    pid: u32,
    name: Option<String>,
    bytes: Bytes,
}

#[derive(Serialize, Deserialize)]
struct Capture {
    millis: u64,
    events_lost: u32,
    buffers_lost: u32,
    undecoded: u64,
    overflow: Bytes,
    rows: Vec<Row>,
}

impl Capture {
    fn render(mut self) -> String {
        let seconds = self.millis.max(1) as f64 / 1000.0;
        let mut output = format!(
            "Windows TCP and UDP activity observed over {seconds:.1} seconds.\n\
            Includes IPv4, IPv6 and local loopback activity. Rates are averages over the capture.\n\
            The report includes process names and IDs when available. Addresses, ports and packet contents are omitted.\n"
        );
        if self.events_lost > 0 || self.buffers_lost > 0 || self.undecoded > 0 {
            let _ = writeln!(
                output,
                "\nPartial capture: Windows reported {} lost events and {} lost buffers. Parsley could not decode {} matching events.",
                self.events_lost, self.buffers_lost, self.undecoded
            );
        }
        if self.rows.is_empty() && self.overflow.total() == 0 {
            output.push_str("\nNo TCP or UDP bytes were observed.");
            return output;
        }
        self.rows
            .sort_by_key(|row| (std::cmp::Reverse(row.bytes.total()), row.pid));
        output.push_str("\nProcess                              PID       Received (B)        Sent (B)   Avg total (B/s)\n");
        let write_row = |output: &mut String, label: &str, pid: &str, bytes: &Bytes| {
            let _ = writeln!(
                output,
                "{label:32} {pid:>7} {:>18} {:>15} {:>17.0}",
                bytes.received,
                bytes.sent,
                bytes.total() as f64 / seconds
            );
        };
        for row in self.rows {
            let label = row.name.as_deref().unwrap_or(if row.pid == 0 {
                "PID unavailable"
            } else {
                "Process name unconfirmed"
            });
            let pid = if row.pid == 0 {
                "—".into()
            } else {
                row.pid.to_string()
            };
            write_row(&mut output, label, &pid, &row.bytes);
        }
        if self.overflow.total() > 0 {
            write_row(
                &mut output,
                "Other process IDs (row limit)",
                "—",
                &self.overflow,
            );
        }
        output
    }
}

fn demo_capture() -> Capture {
    Capture {
        millis: SECONDS * 1000,
        events_lost: 0,
        buffers_lost: 0,
        undecoded: 0,
        overflow: Bytes::default(),
        rows: vec![
            Row {
                pid: 540,
                name: Some("demo-browser.exe".into()),
                bytes: Bytes {
                    received: 8_420_000,
                    sent: 260_000,
                },
            },
            Row {
                pid: 890,
                name: Some("demo-backup.exe".into()),
                bytes: Bytes {
                    received: 110_000,
                    sent: 4_720_000,
                },
            },
            Row {
                pid: 2400,
                name: None,
                bytes: Bytes {
                    received: 146_000,
                    sent: 91_000,
                },
            },
        ],
    }
}

pub fn demo() -> String {
    demo_capture().render()
}

pub(crate) fn interface_capture(demo: bool) -> Result<(serde_json::Value, String), String> {
    let mut capture = if demo {
        demo_capture()
    } else {
        collect_capture()?
    };
    capture
        .rows
        .sort_by_key(|row| (std::cmp::Reverse(row.bytes.total()), row.pid));
    let data = serde_json::to_value(&capture).map_err(|error| error.to_string())?;
    let title = if demo {
        "Fictional bandwidth by process"
    } else {
        "Bandwidth by process"
    };
    Ok((
        data,
        crate::report::render_snapshot(title, &capture.render()),
    ))
}

struct Handle(HANDLE);

impl Handle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

fn pipe_name(id: &str) -> Vec<u16> {
    wide(format!(r"\\.\pipe\Primitive-Parsley-Bandwidth-{id}"))
}

fn server(id: &str) -> io::Result<Handle> {
    Handle::new(unsafe {
        CreateNamedPipeW(
            pipe_name(id).as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            16_384,
            0,
            null(),
        )
    })
}

fn connect(pipe: &Handle) -> io::Result<bool> {
    if unsafe { ConnectNamedPipe(pipe.0, null_mut()) } != 0 {
        return Ok(true);
    }
    match unsafe { GetLastError() } {
        ERROR_PIPE_CONNECTED => Ok(true),
        ERROR_PIPE_LISTENING => Ok(false),
        code => Err(io::Error::from_raw_os_error(code as i32)),
    }
}

fn client(id: &str) -> io::Result<File> {
    let pipe = unsafe {
        CreateFileW(
            pipe_name(id).as_ptr(),
            GENERIC_WRITE,
            0,
            null(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            null_mut(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // The resulting File owns the write-only pipe handle.
        Ok(unsafe { File::from_raw_handle(pipe) })
    }
}

fn worker_id() -> Result<String, String> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| "Could not allocate a unique capture ID.")?;
    Ok(nonce.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn collect() -> Result<String, String> {
    collect_capture().map(Capture::render)
}

fn collect_capture() -> Result<Capture, String> {
    let id = worker_id()?;
    let pipe =
        server(&id).map_err(|error| format!("Could not open a local capture pipe. {error}"))?;
    if connect(&pipe).map_err(|error| format!("Could not listen for the collector. {error}"))? {
        return Err("An unexpected client connected to the capture pipe.".into());
    }
    let executable = wide(std::env::current_exe().map_err(|error| error.to_string())?);
    let parameters = wide(format!("{WORKER} {id}"));
    let mut launch = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE,
        lpVerb: w!("runas"),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    if unsafe { ShellExecuteExW(&mut launch) } == 0 {
        let error = unsafe { GetLastError() };
        return Err(if error == ERROR_CANCELLED {
            "Windows administrator permission was cancelled.".into()
        } else {
            format!("Windows could not open the administrator prompt ({error}).")
        });
    }
    let process = Handle::new(launch.hProcess)
        .map_err(|error| format!("Windows did not return the collector process. {error}"))?;
    read_reply(&pipe, &process)
}

fn read_reply(pipe: &Handle, process: &Handle) -> Result<Capture, String> {
    let expected_pid = unsafe { GetProcessId(process.0) };
    if expected_pid == 0 {
        return Err("Windows could not identify the capture collector.".into());
    }
    let deadline = Instant::now() + Duration::from_secs(SECONDS + 20);
    let mut connected = false;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    while Instant::now() < deadline {
        if !connected {
            connected = connect(pipe).map_err(|error| error.to_string())?;
            if connected {
                let mut pid = 0;
                if unsafe { GetNamedPipeClientProcessId(pipe.0, &mut pid) } == 0
                    || pid != expected_pid
                {
                    return Err("Could not verify the capture collector.".into());
                }
            }
        }
        if connected {
            let mut count = 0;
            let result = unsafe {
                ReadFile(
                    pipe.0,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut count,
                    null_mut(),
                )
            };
            if result == 0 {
                match unsafe { GetLastError() } {
                    ERROR_NO_DATA => {}
                    ERROR_BROKEN_PIPE => break,
                    code => return Err(format!("Could not read the capture pipe ({code}).")),
                }
            } else if count > 0 {
                bytes.extend_from_slice(&buffer[..count as usize]);
                if bytes.len() > MAX_REPLY {
                    return Err("The collector response exceeded its size limit.".into());
                }
                if bytes.last() == Some(&b'\n') {
                    return serde_json::from_slice::<Result<Capture, String>>(&bytes).map_err(
                        |error| format!("Could not read the collector response. {error}"),
                    )?;
                }
                continue;
            }
        }
        if !connected && unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err("The collector ended or timed out before returning a complete capture.".into())
}

pub fn maybe_worker() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != WORKER {
        return None;
    }
    let mut run = || -> Result<(), String> {
        let id = args
            .next()
            .and_then(|value| value.into_string().ok())
            .filter(|id| crate::capture::valid_id(id))
            .ok_or("Invalid bandwidth capture ID.")?;
        if args.next().is_some() {
            return Err("Unexpected bandwidth collector arguments.".into());
        }
        let mut pipe = client(&id).map_err(|error| error.to_string())?;
        send_reply(&mut pipe, trace()).map_err(|error| error.to_string())
    };
    Some(match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Parsley: {error}");
            ExitCode::FAILURE
        }
    })
}

fn send_reply(pipe: &mut File, reply: Result<Capture, String>) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&reply)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_REPLY {
        return Err(io::Error::other(
            "The bandwidth capture response exceeded its size limit.",
        ));
    }
    pipe.write_all(&bytes)?;
    // Keep the sender connected until the reader has verified its PID and
    // consumed the reply. This also covers a collector that fails immediately.
    if unsafe { FlushFileBuffers(pipe.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[repr(C)]
struct TraceProperties {
    properties: EVENT_TRACE_PROPERTIES,
    name: [u16; 64],
}

impl TraceProperties {
    fn new() -> Self {
        let mut properties = EVENT_TRACE_PROPERTIES::default();
        properties.Wnode.BufferSize = size_of::<Self>() as u32;
        properties.Wnode.Guid = SESSION_ID;
        properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        properties.Wnode.ClientContext = 1;
        properties.BufferSize = 64;
        properties.MinimumBuffers = 4;
        properties.MaximumBuffers = 64;
        properties.FlushTimer = 1;
        properties.LogFileMode =
            EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_NO_PER_PROCESSOR_BUFFERING;
        properties.LoggerNameOffset = std::mem::offset_of!(Self, name) as u32;
        Self {
            properties,
            name: [0; 64],
        }
    }
}

struct TraceLock(Handle);

impl TraceLock {
    fn acquire() -> Result<Self, String> {
        let handle = Handle::new(unsafe {
            CreateMutexW(null(), 0, w!("Global\\Primitive-Parsley-Bandwidth"))
        })
        .map_err(|error| format!("Could not coordinate the bandwidth collector. {error}"))?;
        if !matches!(
            unsafe { WaitForSingleObject(handle.0, 0) },
            WAIT_OBJECT_0 | WAIT_ABANDONED
        ) {
            return Err("Another Parsley bandwidth capture is already active.".into());
        }
        Ok(Self(handle))
    }
}

impl Drop for TraceLock {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.0) };
    }
}

struct Session(Option<CONTROLTRACE_HANDLE>);

impl Session {
    fn start() -> Result<Self, String> {
        let mut properties = TraceProperties::new();
        let mut handle = CONTROLTRACE_HANDLE::default();
        let mut status = unsafe { StartTraceW(&mut handle, SESSION, &mut properties.properties) };
        if status == ERROR_ALREADY_EXISTS {
            // A previous worker may have been killed before it could stop. The
            // mutex excludes active Parsley workers. Check the session GUID as well.
            let mut previous = TraceProperties::new();
            let query = unsafe {
                ControlTraceW(
                    CONTROLTRACE_HANDLE::default(),
                    SESSION,
                    &mut previous.properties,
                    EVENT_TRACE_CONTROL_QUERY,
                )
            };
            if query == 0 && same_guid(previous.properties.Wnode.Guid, SESSION_ID) {
                let stop = unsafe {
                    ControlTraceW(
                        CONTROLTRACE_HANDLE::default(),
                        SESSION,
                        &mut previous.properties,
                        EVENT_TRACE_CONTROL_STOP,
                    )
                };
                trace_status("recover the previous Parsley trace", stop)?;
                properties = TraceProperties::new();
                status = unsafe { StartTraceW(&mut handle, SESSION, &mut properties.properties) };
            }
        }
        trace_status("start the network trace", status)?;
        Ok(Self(Some(handle)))
    }

    fn enable(&self) -> Result<(), String> {
        #[repr(C)]
        struct EventFilter {
            include: u8,
            reserved: u8,
            count: u16,
            ids: [u16; EVENT_IDS.len()],
        }
        let filter = EventFilter {
            include: 1,
            reserved: 0,
            count: EVENT_IDS.len() as u16,
            ids: EVENT_IDS,
        };
        let mut descriptor = EVENT_FILTER_DESCRIPTOR {
            Ptr: (&raw const filter) as u64,
            Size: size_of::<EventFilter>() as u32,
            Type: EVENT_FILTER_TYPE_EVENT_ID,
        };
        let parameters = ENABLE_TRACE_PARAMETERS {
            Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
            EnableFilterDesc: &mut descriptor,
            FilterDescCount: 1,
            ..Default::default()
        };
        let result = unsafe {
            EnableTraceEx2(
                self.0.expect("session is active"),
                &PROVIDER,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                4,
                0x30,
                0,
                3000,
                &parameters,
            )
        };
        trace_status("enable TCP and UDP events", result)
    }

    fn stop(&mut self) -> Result<EVENT_TRACE_PROPERTIES, String> {
        let mut properties = TraceProperties::new();
        if let Some(handle) = self.0 {
            let status = unsafe {
                ControlTraceW(
                    handle,
                    null(),
                    &mut properties.properties,
                    EVENT_TRACE_CONTROL_STOP,
                )
            };
            trace_status("stop the network trace", status)?;
            self.0 = None;
        }
        Ok(properties.properties)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn trace_status(action: &str, code: u32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not {action}. {}",
            io::Error::from_raw_os_error(code as i32)
        ))
    }
}

#[derive(Default)]
struct Totals {
    by_pid: BTreeMap<u32, Bytes>,
    overflow: Bytes,
    undecoded: u64,
}

impl Totals {
    fn add(&mut self, id: u16, version: u8, data: &[u8]) {
        if !EVENT_IDS.contains(&id) {
            return;
        }
        let [p0, p1, p2, p3, s0, s1, s2, s3, ..] = data else {
            self.undecoded += 1;
            return;
        };
        if version != 0 {
            self.undecoded += 1;
            return;
        }
        let pid = u32::from_le_bytes([*p0, *p1, *p2, *p3]);
        let size = u64::from(u32::from_le_bytes([*s0, *s1, *s2, *s3]));
        if size == 0 {
            return;
        }
        let bytes = if self.by_pid.len() < MAX_ROWS || self.by_pid.contains_key(&pid) {
            self.by_pid.entry(pid).or_default()
        } else {
            &mut self.overflow
        };
        if matches!(id, 10 | 26 | 42 | 58) {
            bytes.sent += size;
        } else {
            bytes.received += size;
        }
    }
}

fn same_guid(first: GUID, second: GUID) -> bool {
    first.data1 == second.data1
        && first.data2 == second.data2
        && first.data3 == second.data3
        && first.data4 == second.data4
}

unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // ProcessTrace invokes this on its consumer thread. The context remains
    // alive until that thread joins. Only the first eight payload bytes enter Rust.
    let Some(record) = (unsafe { record.as_ref() }) else {
        return;
    };
    if !same_guid(record.EventHeader.ProviderId, PROVIDER) {
        return;
    }
    let totals = unsafe { &mut *record.UserContext.cast::<Totals>() };
    let data = if record.UserDataLength >= 8 && !record.UserData.is_null() {
        unsafe { std::slice::from_raw_parts(record.UserData.cast::<u8>(), 8) }
    } else {
        &[]
    };
    let event = record.EventHeader.EventDescriptor;
    totals.add(event.Id, event.Version, data);
}

pub(crate) fn process_identity(pid: u32) -> Option<(u64, String)> {
    let handle =
        Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }).ok()?;
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    if unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) } == 0
        || exited.dwLowDateTime != 0
        || exited.dwHighDateTime != 0
    {
        return None;
    }
    // Query the image from this same handle to avoid associating a name from
    // a different process after Windows recycles a PID. Keep only the filename.
    let mut name = [0u16; 1024];
    let mut size = name.len() as u32;
    if unsafe {
        windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
            handle.0,
            0,
            name.as_mut_ptr(),
            &mut size,
        )
    } == 0
    {
        return None;
    }
    let image = String::from_utf16_lossy(&name[..size as usize]);
    let label: String = image
        .rsplit(['\\', '/'])
        .next()?
        .chars()
        .filter(|character| !character.is_control())
        .take(96)
        .collect();
    if label.is_empty() {
        return None;
    }
    Some((
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        label,
    ))
}

fn trace() -> Result<Capture, String> {
    let _lock = TraceLock::acquire()?;
    let before: BTreeMap<_, _> = crate::connections::process_names()
        .into_keys()
        .filter_map(|pid| process_identity(pid).map(|identity| (pid, identity)))
        .collect();
    let mut session = Session::start()?;
    let mut totals = Box::<Totals>::default();
    let mut log = EVENT_TRACE_LOGFILEW {
        LoggerName: SESSION.cast_mut(),
        Anonymous1: EVENT_TRACE_LOGFILEW_0 {
            ProcessTraceMode: PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD,
        },
        Anonymous2: EVENT_TRACE_LOGFILEW_1 {
            EventRecordCallback: Some(on_event),
        },
        Context: (&raw mut *totals).cast(),
        ..Default::default()
    };
    let consumer = unsafe { OpenTraceW(&mut log) };
    if consumer.Value == u64::MAX {
        return Err(format!(
            "Windows could not open the network event stream. {}",
            io::Error::last_os_error()
        ));
    }
    let (elapsed, properties) = std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .name("parsley-etw".into())
            .spawn_scoped(scope, move || unsafe {
                ProcessTrace(&consumer, 1, null(), null())
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                unsafe { CloseTrace(consumer) };
                return Err(format!("Could not start the network event reader. {error}"));
            }
        };
        let start = Instant::now();
        let enabled = session.enable();
        if enabled.is_ok() {
            std::thread::sleep(Duration::from_secs(SECONDS).saturating_sub(start.elapsed()));
        }
        let elapsed = start.elapsed();
        let properties = session.stop();
        if properties.is_err() {
            // A failed STOP must not leave ProcessTrace blocking on this reader.
            unsafe { CloseTrace(consumer) };
        }
        let read = thread.join();
        if properties.is_ok() {
            unsafe { CloseTrace(consumer) };
        }
        enabled?;
        let properties = properties?;
        trace_status(
            "read the network event stream",
            read.map_err(|_| "The event reader stopped unexpectedly.")?,
        )?;
        Ok((elapsed, properties))
    })?;
    let rows = std::mem::take(&mut totals.by_pid)
        .into_iter()
        .map(|(pid, bytes)| {
            // Only name a PID whose creation time was verified on both sides of
            // the capture. The report still includes bytes from short-lived processes.
            let name = before.get(&pid).and_then(|identity| {
                (process_identity(pid).as_ref() == Some(identity)).then(|| identity.1.clone())
            });
            Row { pid, name, bytes }
        })
        .collect();
    Ok(Capture {
        millis: elapsed.as_millis() as u64,
        events_lost: properties.EventsLost,
        buffers_lost: properties.RealTimeBuffersLost,
        undecoded: totals.undecoded,
        overflow: totals.overflow,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_pipe_returns_complete_capture_and_immediate_failure() {
        let process = Handle::new(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, std::process::id())
        })
        .unwrap();
        for reply in [
            Ok(Capture {
                millis: 15_000,
                events_lost: 0,
                buffers_lost: 0,
                undecoded: 0,
                overflow: Bytes::default(),
                rows: vec![Row {
                    pid: 901,
                    name: Some("demo-large-transfer.exe".into()),
                    bytes: Bytes {
                        sent: 500,
                        received: 1000,
                    },
                }],
            }),
            Err("Fictional trace failure".into()),
        ] {
            let succeeded = reply.is_ok();
            let id = worker_id().unwrap();
            let pipe = server(&id).unwrap();
            assert!(!connect(&pipe).unwrap());
            let sender = std::thread::spawn(move || send_reply(&mut client(&id).unwrap(), reply));
            match read_reply(&pipe, &process) {
                Ok(capture) => {
                    assert!(succeeded);
                    assert!(capture.render().contains("demo-large-transfer.exe"));
                }
                Err(error) => {
                    assert!(!succeeded, "{error}");
                    assert_eq!(error, "Fictional trace failure");
                }
            }
            drop(pipe);
            sender.join().unwrap().unwrap();
        }
    }

    #[test]
    fn aggregates_only_matching_send_and_receive_metadata() {
        let mut totals = Totals::default();
        let mut data = 901_u32.to_le_bytes().to_vec();
        data.extend(250_u32.to_le_bytes());
        data.extend(b"ignored address and ports");
        for id in EVENT_IDS {
            totals.add(id, 0, &data);
        }
        totals.add(77, 0, &data);
        totals.add(10, 1, &data);
        totals.add(11, 0, &data[..7]);
        assert_eq!(totals.by_pid.len(), 1);
        assert_eq!(totals.by_pid[&901].sent, 1000);
        assert_eq!(totals.by_pid[&901].received, 1000);
        assert_eq!(totals.undecoded, 2);
    }

    #[test]
    fn limits_rows_without_losing_byte_totals() {
        let mut totals = Totals::default();
        for pid in 1..=MAX_ROWS as u32 + 1 {
            let mut data = pid.to_le_bytes().to_vec();
            data.extend(100_u32.to_le_bytes());
            totals.add(42, 0, &data);
        }
        assert_eq!(totals.by_pid.len(), MAX_ROWS);
        assert_eq!(totals.overflow.sent, 100);
    }

    #[test]
    fn labels_partial_and_unattributed_results() {
        let capture = Capture {
            millis: 15000,
            events_lost: 8,
            buffers_lost: 2,
            undecoded: 1,
            overflow: Bytes::default(),
            rows: vec![Row {
                pid: 901,
                name: None,
                bytes: Bytes {
                    sent: 0,
                    received: 3000,
                },
            }],
        };
        let report = capture.render();
        assert!(report.contains("Partial capture"));
        assert!(report.contains("Process name unconfirmed"));
        assert!(report.contains("3000"));
        assert!(report.contains("200\n"));
    }
}
