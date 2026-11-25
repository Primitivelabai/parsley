use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null;

use chrono::{DateTime, SecondsFormat, Utc};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, GetLastError,
};
use windows_sys::Win32::System::EventLog::*;

use crate::incidents::{Detail, Event, Kind};

const MAX_RECORDS_PER_SOURCE: usize = 5_000;
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const EVENT_IDS: &str = "(EventID=1 and Provider[@Name='Microsoft-Windows-Power-Troubleshooter']) or EventID=17 or EventID=18 or EventID=19 or EventID=41 or EventID=1000 or EventID=1001 or EventID=1002 or EventID=1074 or EventID=4101 or EventID=7031 or EventID=7034";

struct RenderContexts {
    system: Handle,
    details: Handle,
    legacy_details: Handle,
}

struct Handle(EVT_HANDLE);

impl Handle {
    fn new(handle: EVT_HANDLE) -> io::Result<Self> {
        if handle == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // Each wrapper owns one handle returned by the Event Log API.
        unsafe { EvtClose(self.0) };
    }
}

pub fn collect(
    file: Option<&Path>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Result<(Vec<Event>, Vec<String>), String> {
    let query = match (start, end) {
        (Some(start), Some(end)) => format!(
            "*[System[({EVENT_IDS}) and TimeCreated[@SystemTime >= '{}' and @SystemTime <= '{}']]]",
            start.to_rfc3339_opts(SecondsFormat::Millis, true),
            end.to_rfc3339_opts(SecondsFormat::Millis, true)
        ),
        _ => format!("*[System[({EVENT_IDS})]]"),
    };
    let context = render_contexts()
        .map_err(|error| format!("Could not initialise Windows Event Log. {error}"))?;
    let mut events = Vec::new();
    let mut notices = Vec::new();
    if let Some(file) = file {
        let limited = read_source(
            &context,
            file.as_os_str(),
            &query,
            EvtQueryFilePath,
            &mut events,
        )
        .map_err(|error| format!("Could not read the event log file. {error}"))?;
        if limited {
            notices.push("The import reached its record limit. Choose a narrower time window for complete results.".into());
        }
    } else {
        let mut successful_sources = 0;
        for channel in ["System", "Application"] {
            let mut channel_events = Vec::new();
            match read_source(
                &context,
                OsStr::new(channel),
                &query,
                EvtQueryChannelPath,
                &mut channel_events,
            ) {
                Ok(limited) => {
                    successful_sources += 1;
                    events.extend(channel_events);
                    if limited {
                        notices.push(format!("The {channel} log reached its record limit. Choose a narrower time window."));
                    }
                }
                Err(error) => notices.push(format!("The {channel} log could not be read. {error}")),
            }
        }
        if successful_sources == 0 {
            return Err(notices.join(" "));
        }
    }
    Ok((events, notices))
}

pub fn evidence(
    channel: &str,
    record: u64,
    file: Option<&Path>,
    expected_at: Option<DateTime<Utc>>,
) -> Result<String, String> {
    let query = format!("*[System[Channel='{channel}' and EventRecordID={record}]]");
    let query: Vec<_> = query.encode_utf16().chain([0]).collect();
    let path = file.map_or(OsStr::new(channel), Path::as_os_str);
    let path: Vec<_> = path.encode_wide().chain([0]).collect();
    let flags = if file.is_some() {
        EvtQueryFilePath
    } else {
        EvtQueryChannelPath
    };
    // The session is local. The caller validated the channel and record ID.
    let result = Handle::new(unsafe { EvtQuery(0, path.as_ptr(), query.as_ptr(), flags) })
        .map_err(|error| format!("Could not query the event record. {error}"))?;
    let mut raw_event = 0;
    let mut returned = 0;
    // Windows writes at most one event handle.
    if unsafe { EvtNext(result.0, 1, &mut raw_event, 3_000, 0, &mut returned) } == 0 {
        let status = unsafe { GetLastError() };
        return Err(if status == ERROR_NO_MORE_ITEMS {
            "That event record was not found in the selected log.".into()
        } else {
            format!(
                "Could not read the event. {}",
                io::Error::from_raw_os_error(status as i32)
            )
        });
    }
    let event = Handle::new(raw_event).map_err(|error| error.to_string())?;
    if let Some(expected) = expected_at {
        let system =
            Handle::new(unsafe { EvtCreateRenderContext(0, null(), EvtRenderContextSystem) })
                .map_err(|error| format!("Could not confirm the Windows event time. {error}"))?;
        let fields = render_fields(&system, &event)
            .map_err(|error| format!("Could not confirm the Windows event time. {error}"))?;
        if fields.created_at() != Some(expected) {
            return Err("Windows assigned that record number to a different event. The original event is no longer at that reference.".into());
        }
    }
    let mut used = 0;
    let mut properties = 0;
    // A zero-sized buffer asks Windows for the required XML buffer length.
    unsafe {
        EvtRender(
            0,
            event.0,
            EvtRenderEventXml,
            0,
            std::ptr::null_mut(),
            &mut used,
            &mut properties,
        )
    };
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || used as usize > MAX_EVENT_BYTES {
        return Err("Windows did not return the size of a supported event record.".into());
    }
    let mut xml = vec![0u16; (used as usize).div_ceil(size_of::<u16>())];
    // The allocated UTF-16 buffer provides at least the requested byte count.
    if unsafe {
        EvtRender(
            0,
            event.0,
            EvtRenderEventXml,
            used,
            xml.as_mut_ptr().cast(),
            &mut used,
            &mut properties,
        )
    } == 0
    {
        return Err(format!(
            "Could not render event XML. {}",
            io::Error::last_os_error()
        ));
    }
    let units = xml
        .get(..used as usize / 2)
        .ok_or("Windows returned an invalid event XML length.")?;
    let units = units
        .strip_suffix(&[0])
        .ok_or("Windows returned unterminated event XML.")?;
    let xml =
        String::from_utf16(units).map_err(|_| "Windows returned invalid UTF-16 event XML.")?;
    Ok(xml
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\r' | '\n' | '\t') {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect())
}

fn render_contexts() -> io::Result<RenderContexts> {
    // The array points to terminated strings that live through the call.
    let paths = [
        windows_sys::w!("Event/EventData/Data[@Name='AppName']"),
        windows_sys::w!("Event/EventData/Data[@Name='ModuleName']"),
        windows_sys::w!("Event/EventData/Data[@Name='ExceptionCode']"),
        windows_sys::w!("Event/EventData/Data[@Name='BugcheckCode']"),
        windows_sys::w!("Event/EventData/Data[@Name='param1']"),
    ];
    Ok(RenderContexts {
        system: Handle::new(unsafe { EvtCreateRenderContext(0, null(), EvtRenderContextSystem) })?,
        details: Handle::new(unsafe {
            EvtCreateRenderContext(paths.len() as u32, paths.as_ptr(), EvtRenderContextValues)
        })?,
        legacy_details: Handle::new(unsafe {
            EvtCreateRenderContext(0, null(), EvtRenderContextUser)
        })?,
    })
}

fn read_source(
    context: &RenderContexts,
    source: &OsStr,
    query: &str,
    flags: u32,
    events: &mut Vec<Event>,
) -> io::Result<bool> {
    let source = source.encode_wide().chain([0]).collect::<Vec<_>>();
    let query = query.encode_utf16().chain([0]).collect::<Vec<_>>();
    // A null session selects the local machine. Both input strings are terminated
    // and live through the call. This function also consumes the query on this thread.
    let result = Handle::new(unsafe {
        EvtQuery(
            0,
            source.as_ptr(),
            query.as_ptr(),
            flags | EvtQueryReverseDirection,
        )
    })?;
    for index in 0..=MAX_RECORDS_PER_SOURCE {
        let mut event = 0;
        let mut returned = 0;
        // The API can write one event handle and one count.
        if unsafe { EvtNext(result.0, 1, &mut event, 3_000, 0, &mut returned) } == 0 {
            if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                return Ok(false);
            }
            return Err(io::Error::last_os_error());
        }
        let event = Handle::new(event)?;
        if index == MAX_RECORDS_PER_SOURCE {
            return Ok(true);
        }
        if let Some(event) = render_event(context, &event)? {
            events.push(event);
        }
    }
    unreachable!("the loop checks for the record limit");
}

struct Fields {
    buffer: Vec<EVT_VARIANT>,
    used: usize,
    count: usize,
}

impl Fields {
    fn created_at(&self) -> Option<DateTime<Utc>> {
        let value = self.at(EvtSystemTimeCreated as usize)?;
        if value.Type != EvtVarTypeFileTime as u32 {
            return None;
        }
        // The event type tag selects the FILETIME member.
        let ticks = unsafe { value.Anonymous.FileTimeVal };
        let unix_ticks = i128::from(ticks) - 116_444_736_000_000_000;
        DateTime::from_timestamp(
            unix_ticks.div_euclid(10_000_000) as i64,
            (unix_ticks.rem_euclid(10_000_000) * 100) as u32,
        )
    }

    fn at(&self, index: usize) -> Option<&EVT_VARIANT> {
        (index < self.count).then(|| &self.buffer[index])
    }

    fn text(&self, index: usize) -> Option<String> {
        let value = self.at(index)?;
        if value.Type != EvtVarTypeString as u32 {
            return None;
        }
        // The type tag identifies the active union member.
        let pointer = unsafe { value.Anonymous.StringVal };
        copy_string(&self.buffer, self.used, pointer)
    }

    fn u32(&self, index: usize) -> Option<u32> {
        let value = self.at(index)?;
        if value.Type == EvtVarTypeUInt32 as u32 {
            // The type tag identifies the active union member.
            return Some(unsafe { value.Anonymous.UInt32Val });
        }
        self.text(index)?.trim().parse().ok()
    }
}

fn render_fields(context: &Handle, event: &Handle) -> io::Result<Fields> {
    let mut used = 0;
    let mut count = 0;
    // A zero-sized buffer asks Windows for the required size in bytes.
    unsafe {
        EvtRender(
            context.0,
            event.0,
            EvtRenderEventValues,
            0,
            std::ptr::null_mut(),
            &mut used,
            &mut count,
        )
    };
    let status = unsafe { GetLastError() };
    if status != ERROR_INSUFFICIENT_BUFFER {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    if used as usize > MAX_EVENT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Event exceeds the supported size",
        ));
    }
    let mut buffer =
        vec![EVT_VARIANT::default(); (used as usize).div_ceil(size_of::<EVT_VARIANT>())];
    // The buffer is aligned for EVT_VARIANT and has at least the requested capacity.
    if unsafe {
        EvtRender(
            context.0,
            event.0,
            EvtRenderEventValues,
            used,
            buffer.as_mut_ptr().cast(),
            &mut used,
            &mut count,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if count as usize > buffer.len()
        || (count as usize) * size_of::<EVT_VARIANT>() > used as usize
        || used as usize > buffer.len() * size_of::<EVT_VARIANT>()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Incomplete Windows event fields",
        ));
    }
    Ok(Fields {
        buffer,
        used: used as usize,
        count: count as usize,
    })
}

fn render_event(context: &RenderContexts, event: &Handle) -> io::Result<Option<Event>> {
    let fields = render_fields(&context.system, event)?;
    let (Some(id), Some(at), Some(record)) = (
        fields.at(EvtSystemEventID as usize),
        fields.created_at(),
        fields.at(EvtSystemEventRecordId as usize),
    ) else {
        return Ok(None);
    };
    if id.Type != EvtVarTypeUInt16 as u32 || record.Type != EvtVarTypeUInt64 as u32 {
        return Ok(None);
    }
    // The type tags above select the active members.
    let (id, record) = unsafe { (id.Anonymous.UInt16Val, record.Anonymous.UInt64Val) };
    let Some(kind) = fields
        .text(EvtSystemProviderName as usize)
        .as_deref()
        .and_then(|provider| Kind::from_event(provider, id))
    else {
        return Ok(None);
    };
    if fields.text(EvtSystemChannel as usize).as_deref() != Some(kind.channel()) {
        return Ok(None);
    }
    let detail = match kind {
        Kind::Crash | Kind::Hang | Kind::Restart | Kind::Bugcheck => {
            render_details(context, event, kind).unwrap_or_default()
        }
        _ => Detail::default(),
    };
    Ok(Some(Event {
        at,
        record,
        kind,
        id,
        detail,
    }))
}

fn render_details(context: &RenderContexts, event: &Handle, kind: Kind) -> io::Result<Detail> {
    let fields = render_fields(&context.details, event)?;
    let mut detail = Detail::default();
    match kind {
        Kind::Crash | Kind::Hang => {
            detail.application = fields.text(0).as_deref().and_then(Detail::filename);
            detail.module = fields.text(1).as_deref().and_then(Detail::filename);
            detail.exception = fields.text(2).as_deref().and_then(Detail::hex);
            if detail.application.is_none() {
                // Older Application Error and Application Hang payloads omit field
                // names. Their first insertion string is the application name.
                // Only use positional crash fields for the known legacy shape.
                let legacy = render_fields(&context.legacy_details, event)?;
                let application = legacy.text(0).as_deref().and_then(Detail::filename);
                if application
                    .as_ref()
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with(".exe"))
                {
                    detail.application = application;
                    if kind == Kind::Crash {
                        detail.module = legacy
                            .text(3)
                            .as_deref()
                            .and_then(Detail::filename)
                            .filter(|name| {
                                let name = name.to_ascii_lowercase();
                                name.ends_with(".dll") || name.ends_with(".exe")
                            });
                        detail.exception = legacy
                            .text(6)
                            .as_deref()
                            .filter(|value| value.len() == 8)
                            .and_then(Detail::hex);
                    }
                }
            }
        }
        Kind::Restart => detail.stop_code = fields.u32(3).filter(|code| *code != 0),
        Kind::Bugcheck => {
            detail.stop_code = fields
                .text(4)
                .as_deref()
                .and_then(|value| value.split_whitespace().next())
                .and_then(Detail::hex)
                .filter(|code| *code != 0);
        }
        _ => {}
    }
    Ok(detail)
}

fn copy_string(buffer: &[EVT_VARIANT], used: usize, value: *const u16) -> Option<String> {
    let offset = value.addr().checked_sub(buffer.as_ptr().addr())?;
    if offset >= used || !offset.is_multiple_of(size_of::<u16>()) {
        return None;
    }
    let len = (used - offset) / size_of::<u16>();
    // The checked offset and length keep the read within the populated buffer.
    // Derive the pointer from the buffer allocation.
    let units = unsafe {
        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>().add(offset).cast::<u16>(), len)
    };
    let terminator = units.iter().position(|unit| *unit == 0)?;
    String::from_utf16(&units[..terminator]).ok()
}
