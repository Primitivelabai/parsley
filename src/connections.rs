use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::fmt::Write;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, FILETIME, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;

#[derive(Default, PartialEq, Eq, Debug)]
struct Counts {
    established: usize,
    listeners: usize,
    udp: usize,
}

#[derive(Clone, Copy)]
struct Layout {
    offset: usize,
    size: usize,
    pid: usize,
    state: Option<usize>,
}

macro_rules! layout {
    ($table:ty, $row:ty, $state:expr) => {
        Layout {
            offset: std::mem::offset_of!($table, table),
            size: size_of::<$row>(),
            pid: std::mem::offset_of!($row, dwOwningPid),
            state: $state,
        }
    };
}

const TCP_V4: Layout = layout!(
    MIB_TCPTABLE_OWNER_PID,
    MIB_TCPROW_OWNER_PID,
    Some(std::mem::offset_of!(MIB_TCPROW_OWNER_PID, dwState))
);
const TCP_V6: Layout = layout!(
    MIB_TCP6TABLE_OWNER_PID,
    MIB_TCP6ROW_OWNER_PID,
    Some(std::mem::offset_of!(MIB_TCP6ROW_OWNER_PID, dwState))
);

pub fn snapshot() -> Result<String, String> {
    let mut by_process = BTreeMap::new();
    let mut unavailable = Vec::new();
    let names = process_names();
    for (name, family, table, tcp) in [
        ("IPv4 TCP", AF_INET, TCP_V4, true),
        ("IPv6 TCP", AF_INET6, TCP_V6, true),
        (
            "IPv4 UDP",
            AF_INET,
            layout!(MIB_UDPTABLE_OWNER_PID, MIB_UDPROW_OWNER_PID, None),
            false,
        ),
        (
            "IPv6 UDP",
            AF_INET6,
            layout!(MIB_UDP6TABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID, None),
            false,
        ),
    ] {
        let result = read_table(
            |buffer, bytes| {
                // The family and class pair select the layout above. The supplied
                // buffer has the length documented in bytes, or is null for sizing.
                unsafe {
                    if tcp {
                        GetExtendedTcpTable(
                            buffer,
                            bytes,
                            0,
                            family.into(),
                            TCP_TABLE_OWNER_PID_ALL,
                            0,
                        )
                    } else {
                        GetExtendedUdpTable(buffer, bytes, 0, family.into(), UDP_TABLE_OWNER_PID, 0)
                    }
                }
            },
            |bytes| count_table(bytes, table, &mut by_process),
        );
        if result.is_err() {
            unavailable.push(name);
        }
    }
    if unavailable.len() == 4 {
        return Err("Windows did not return any process connection tables.".into());
    }
    let mut output = String::from(
        "Counts describe sockets at the moment of the snapshot. IP addresses and ports are omitted.\n\n",
    );
    if !unavailable.is_empty() {
        let _ = writeln!(
            output,
            "Windows did not return {}. Counts may be incomplete.\n",
            unavailable.join(", ")
        );
    }
    if by_process.is_empty() {
        output.push_str("Windows returned no matching endpoints.");
        return Ok(output);
    }
    let mut processes: Vec<_> = by_process.into_iter().collect();
    processes.sort_by_key(|(_, counts)| {
        std::cmp::Reverse(counts.established + counts.listeners + counts.udp)
    });
    let _ = writeln!(
        output,
        "{:<36} {:>8} {:>16} {:>14} {:>14}",
        "Process", "PID", "Established TCP", "TCP listeners", "UDP endpoints"
    );
    for (pid, counts) in processes {
        let name = names.get(&pid).map_or("Unknown process", String::as_str);
        let _ = writeln!(
            output,
            "{name:<36.36} {pid:>8} {:>16} {:>14} {:>14}",
            counts.established, counts.listeners, counts.udp
        );
    }
    Ok(output)
}

fn read_table<T>(
    query: impl Fn(*mut c_void, *mut u32) -> u32,
    parse: impl FnOnce(&[u8]) -> io::Result<T>,
) -> io::Result<T> {
    let mut needed = 0;
    let first = query(std::ptr::null_mut(), &mut needed);
    if first != ERROR_INSUFFICIENT_BUFFER {
        return Err(io::Error::from_raw_os_error(first as i32));
    }
    for _ in 0..4 {
        const MAX_TABLE: u32 = 16 * 1024 * 1024;
        if needed == 0 || needed > MAX_TABLE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported network table size",
            ));
        }
        let mut buffer = vec![0u32; (needed as usize).div_ceil(size_of::<u32>())];
        let status = query(buffer.as_mut_ptr().cast(), &mut needed);
        if status == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let available = buffer.len() * size_of::<u32>();
        if needed as usize > available {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Truncated network table",
            ));
        }
        // The array is aligned for these Windows layouts and was initialised;
        // parsing byte slices avoids constructing references to arbitrary rows.
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), needed as usize) };
        return parse(bytes);
    }
    Err(io::Error::new(
        io::ErrorKind::Interrupted,
        "The network table changed during capture",
    ))
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid network table")
}

fn word(bytes: &[u8], index: usize) -> io::Result<u32> {
    bytes
        .get(index..index + size_of::<u32>())
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_ne_bytes)
        .ok_or_else(invalid)
}

fn table_rows(bytes: &[u8], table: Layout) -> io::Result<&[u8]> {
    let count = word(bytes, 0)? as usize;
    let end = count
        .checked_mul(table.size)
        .and_then(|length| length.checked_add(table.offset))
        .ok_or_else(invalid)?;
    bytes.get(table.offset..end).ok_or_else(invalid)
}

fn count_table(bytes: &[u8], table: Layout, out: &mut BTreeMap<u32, Counts>) -> io::Result<()> {
    for row in table_rows(bytes, table)?.chunks_exact(table.size) {
        let pid = word(row, table.pid)?;
        if pid == 0 {
            continue;
        }
        if let Some(state) = table.state {
            match word(row, state)? as i32 {
                MIB_TCP_STATE_ESTAB => out.entry(pid).or_default().established += 1,
                MIB_TCP_STATE_LISTEN => out.entry(pid).or_default().listeners += 1,
                _ => {}
            }
        } else {
            out.entry(pid).or_default().udp += 1;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Peer {
    pid: u32,
    remote: SocketAddr,
}

fn peer_rows(bytes: &[u8], ipv6: bool) -> io::Result<Vec<Peer>> {
    let table = if ipv6 { TCP_V6 } else { TCP_V4 };
    let mut peers = Vec::new();
    for row in table_rows(bytes, table)?.chunks_exact(table.size) {
        let pid = word(row, table.pid)?;
        if pid == 0
            || word(row, table.state.expect("TCP state offset"))? as i32 != MIB_TCP_STATE_ESTAB
        {
            continue;
        }
        let remote = if ipv6 {
            let offset = std::mem::offset_of!(MIB_TCP6ROW_OWNER_PID, ucRemoteAddr);
            let address: [u8; 16] = row[offset..offset + 16].try_into().map_err(|_| invalid())?;
            let port = word(
                row,
                std::mem::offset_of!(MIB_TCP6ROW_OWNER_PID, dwRemotePort),
            )?;
            let scope = word(
                row,
                std::mem::offset_of!(MIB_TCP6ROW_OWNER_PID, dwRemoteScopeId),
            )?;
            let address = Ipv6Addr::from(address);
            match address.to_ipv4_mapped() {
                Some(mapped) => SocketAddr::new(mapped.into(), u16::from_be(port as u16)),
                None => {
                    SocketAddrV6::new(address, u16::from_be(port as u16), 0, u32::from_be(scope))
                        .into()
                }
            }
        } else {
            let address = word(
                row,
                std::mem::offset_of!(MIB_TCPROW_OWNER_PID, dwRemoteAddr),
            )?;
            let port = word(
                row,
                std::mem::offset_of!(MIB_TCPROW_OWNER_PID, dwRemotePort),
            )?;
            SocketAddr::new(
                Ipv4Addr::from(address.to_ne_bytes()).into(),
                u16::from_be(port as u16),
            )
        };
        if remote.port() > 0
            && !remote.ip().is_loopback()
            && !remote.ip().is_unspecified()
            && !remote.ip().is_multicast()
        {
            peers.push(Peer { pid, remote });
        }
    }
    Ok(peers)
}

fn read_peers(ipv6: bool) -> io::Result<Vec<Peer>> {
    read_table(
        |buffer, bytes| {
            let family = if ipv6 { AF_INET6 } else { AF_INET };
            // Windows writes the selected address-family table into this buffer.
            unsafe {
                GetExtendedTcpTable(
                    buffer,
                    bytes,
                    0,
                    family.into(),
                    TCP_TABLE_OWNER_PID_CONNECTIONS,
                    0,
                )
            }
        },
        |bytes| peer_rows(bytes, ipv6),
    )
}

type DnsNames = BTreeMap<IpAddr, BTreeSet<String>>;

struct AppDns {
    image: String,
    at: u64,
    name: String,
}

#[derive(Default)]
struct NetworkContext {
    dns: DnsNames,
    app_dns: BTreeMap<(u32, IpAddr), Vec<AppDns>>,
    services: BTreeMap<u32, BTreeSet<String>>,
}

fn domain_name(name: &str) -> Option<String> {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let valid = name.len() <= 253
        && name.contains('.')
        && name.parse::<IpAddr>().is_err()
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    valid.then_some(name)
}

impl NetworkContext {
    fn destination_name(
        &self,
        peer: Peer,
        owner: &Owner,
        last_seen: u64,
    ) -> Option<(&str, &'static str)> {
        let address = peer.remote.ip();
        if let Owner::Verified(created, image) = owner {
            let mut matches = self
                .app_dns
                .get(&(peer.pid, address))
                .into_iter()
                .flatten()
                .filter(|query| {
                    (*created..=last_seen).contains(&query.at)
                        && query.image.eq_ignore_ascii_case(image)
                })
                .map(|query| query.name.as_str());
            if let Some(name) = matches.next()
                && matches.all(|other| other == name)
            {
                return Some((name, "app-dns"));
            }
        }
        let names = self.dns.get(&address)?;
        if names.len() == 1 {
            return names.first().map(|name| (name.as_str(), "windows-cache"));
        }
        None
    }
}

fn network_operator(address: IpAddr) -> Option<&'static str> {
    let mut operator = None;
    for (name, cidr) in include_str!("network-operators.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once(' '))
    {
        let Some((network, prefix)) = cidr.split_once('/') else {
            continue;
        };
        let (Ok(network), Ok(prefix)) = (network.parse::<IpAddr>(), prefix.parse::<u32>()) else {
            continue;
        };
        let matches = match (address.to_canonical(), network) {
            (IpAddr::V4(address), IpAddr::V4(network)) if (1..=32).contains(&prefix) => {
                (u32::from(address) ^ u32::from(network)) >> (32 - prefix) == 0
            }
            (IpAddr::V6(address), IpAddr::V6(network)) if (1..=128).contains(&prefix) => {
                (u128::from(address) ^ u128::from(network)) >> (128 - prefix) == 0
            }
            _ => false,
        };
        if matches {
            if operator.is_some_and(|previous| previous != name) {
                return None;
            }
            operator = Some(name);
        }
    }
    operator
}

fn network_context() -> NetworkContext {
    let data = crate::system::run_script(
        r#"$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$dns = @()
$services = @()
try { $dns = @(Get-DnsClientCache | Select-Object -First 2048 Entry,Name,Data) } catch {}
try { $services = @(Get-CimInstance Win32_Service -Filter 'ProcessId > 0' | Select-Object -First 512 ProcessId,DisplayName) } catch {}
$dnsEvents = @()
try {
    $channel = 'Microsoft-Windows-Sysmon/Operational'
    $log = Get-WinEvent -ListLog $channel
    if ($log.IsEnabled) {
        $records = @(Get-WinEvent -FilterHashtable @{LogName=$channel;Id=22;StartTime=(Get-Date).AddMinutes(-5)} -MaxEvents 512 -ErrorAction SilentlyContinue)
        $dnsEvents = @(foreach ($event in $records) {
            $values = @{}
            foreach ($field in ([xml]$event.ToXml()).Event.EventData.Data) {
                $values[[string]$field.Name] = [string]$field.'#text'
            }
            if ($values.QueryStatus -ne '0' -or !$values.ProcessId -or !$values.Image -or !$event.TimeCreated) { continue }
            [pscustomobject]@{
                Pid = [uint32]$values.ProcessId
                Image = [IO.Path]::GetFileName($values.Image)
                At = $event.TimeCreated.ToUniversalTime().ToFileTimeUtc()
                Name = $values.QueryName
                Results = $values.QueryResults
            }
        })
    }
} catch {}
ConvertTo-Json -InputObject @{dns=$dns;services=$services;dnsEvents=$dnsEvents} -Depth 4 -Compress"#,
    );
    let Some(data) = data
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
    else {
        return NetworkContext::default();
    };
    let mut context = NetworkContext::default();
    for row in data["dns"].as_array().into_iter().flatten() {
        let Some(address) = row["Data"]
            .as_str()
            .and_then(|value| value.parse::<IpAddr>().ok())
        else {
            continue;
        };
        for name in [row["Entry"].as_str(), row["Name"].as_str()]
            .into_iter()
            .flatten()
            .filter_map(domain_name)
        {
            context
                .dns
                .entry(address.to_canonical())
                .or_default()
                .insert(name);
        }
    }
    for row in data["dnsEvents"].as_array().into_iter().flatten() {
        let (Some(pid), Some(image), Some(at), Some(name), Some(results)) = (
            row["Pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()),
            row["Image"].as_str(),
            row["At"].as_u64(),
            row["Name"].as_str().and_then(domain_name),
            row["Results"].as_str(),
        ) else {
            continue;
        };
        for address in results
            .split(';')
            .take(32)
            .filter_map(|part| part.trim().parse::<IpAddr>().ok())
        {
            context
                .app_dns
                .entry((pid, address.to_canonical()))
                .or_default()
                .push(AppDns {
                    image: image.to_owned(),
                    at,
                    name: name.clone(),
                });
        }
    }
    for row in data["services"].as_array().into_iter().flatten() {
        if let (Some(pid), Some(name)) = (
            row["ProcessId"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok()),
            row["DisplayName"].as_str(),
        ) {
            let label: String = name
                .chars()
                .filter(|character| !character.is_control())
                .take(120)
                .collect();
            if !label.is_empty() {
                context.services.entry(pid).or_default().insert(label);
            }
        }
    }
    context
}

fn possible_purpose(hostname: &str) -> Option<&'static str> {
    match hostname {
        "v10.events.data.microsoft.com" | "self.events.data.microsoft.com" => {
            Some("Possible Windows diagnostics")
        }
        "telecommand.telemetry.microsoft.com" | "www.telecommandsvc.microsoft.com" => {
            Some("Possible Windows error reporting")
        }
        "msedge.api.cdp.microsoft.com" => Some("Possible Edge updates"),
        name if name.ends_with(".update.microsoft.com")
            || name.ends_with(".dl.delivery.mp.microsoft.com") =>
        {
            Some("Possible Windows or Store updates")
        }
        _ => None,
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Observation {
    samples: u8,
    seconds: u64,
    incomplete: bool,
    named_destinations: bool,
    omitted: bool,
    rows: Vec<ObservedPeer>,
}

#[derive(serde::Serialize)]
struct ObservedPeer {
    pid: u32,
    app: Option<String>,
    name_source: &'static str,
    services: Vec<String>,
    address: SocketAddr,
    possible_name: Option<String>,
    domain_source: Option<&'static str>,
    operator: Option<&'static str>,
    purpose: Option<&'static str>,
    seen: u8,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    Verified(u64, String),
    Listed(String),
    Unknown,
}

impl Owner {
    fn name(&self) -> Option<&str> {
        match self {
            Self::Verified(_, name) | Self::Listed(name) => Some(name),
            Self::Unknown => None,
        }
    }
}

fn owner_at(
    pid: u32,
    at: u64,
    before: &BTreeMap<u32, String>,
    after: &BTreeMap<u32, String>,
) -> Owner {
    match crate::network::process_identity(pid) {
        Some((created, name)) if created <= at => Owner::Verified(created, name),
        Some(_) => Owner::Unknown,
        None => before
            .get(&pid)
            .filter(|name| {
                !name.is_empty()
                    && after
                        .get(&pid)
                        .is_some_and(|other| other.eq_ignore_ascii_case(name))
            })
            .map_or(Owner::Unknown, |name| Owner::Listed(name.clone())),
    }
}

pub(crate) fn observe(demo: bool) -> Result<Observation, String> {
    const SAMPLES: u8 = 5;
    const MAX_OBSERVED: usize = 4096;
    if demo {
        return Ok(Observation {
            samples: SAMPLES,
            seconds: 8,
            incomplete: false,
            named_destinations: true,
            omitted: false,
            rows: vec![
                ObservedPeer {
                    pid: 1104,
                    app: Some("demo-browser.exe".into()),
                    name_source: "verified",
                    services: vec![],
                    address: SocketAddr::new(Ipv4Addr::new(192, 0, 2, 8).into(), 443),
                    possible_name: Some("example.net".into()),
                    domain_source: Some("app-dns"),
                    operator: Some("Example network"),
                    purpose: None,
                    seen: 4,
                },
                ObservedPeer {
                    pid: 2088,
                    app: Some("demo-service.exe".into()),
                    name_source: "windows-list",
                    services: vec!["Example Windows service".into()],
                    address: SocketAddr::new(Ipv4Addr::new(198, 51, 100, 42).into(), 443),
                    possible_name: Some("v10.events.data.microsoft.com".into()),
                    domain_source: Some("windows-cache"),
                    operator: None,
                    purpose: possible_purpose("v10.events.data.microsoft.com"),
                    seen: 2,
                },
            ],
        });
    }
    let start = Instant::now();
    let mut totals = BTreeMap::new();
    let mut samples = 0;
    let mut incomplete = false;
    let mut omitted = false;
    for index in 0..SAMPLES {
        if index > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        let mut at = FILETIME::default();
        // Read the timestamp before reading either table to reject subsequent PID reuse.
        unsafe { GetSystemTimeAsFileTime(&mut at) };
        let at = (u64::from(at.dwHighDateTime) << 32) | u64::from(at.dwLowDateTime);
        let before = process_names();
        let mut observed = BTreeSet::new();
        let mut available = false;
        for ipv6 in [false, true] {
            match read_peers(ipv6) {
                Ok(peers) => {
                    available = true;
                    observed.extend(peers);
                }
                Err(_) => incomplete = true,
            }
        }
        let after = process_names();
        samples += u8::from(available);
        let mut identities = BTreeMap::new();
        for peer in observed {
            let identity = identities
                .entry(peer.pid)
                .or_insert_with(|| owner_at(peer.pid, at, &before, &after));
            let key = (peer, identity.clone());
            if totals.len() < MAX_OBSERVED || totals.contains_key(&key) {
                let (seen, last, last_seen) = totals.entry(key).or_insert((0, index, at));
                *seen += 1;
                *last = index;
                *last_seen = at;
            } else {
                omitted = true;
            }
        }
    }
    if samples == 0 {
        return Err("Windows did not provide its active TCP connections.".into());
    }
    let seconds = start.elapsed().as_secs();
    let context = network_context();
    let current_names = process_names();
    let mut rows: Vec<_> = totals
        .into_iter()
        .map(|((peer, owner), (seen, last, last_seen))| {
            let (possible_name, domain_source) = context
                .destination_name(peer, &owner, last_seen)
                .map_or((None, None), |(name, source)| (Some(name), Some(source)));
            let purpose = possible_name.and_then(possible_purpose);
            let services = context
                .services
                .get(&peer.pid)
                .filter(|_| last == SAMPLES - 1)
                .filter(|_| match &owner {
                    Owner::Verified(created, name) => crate::network::process_identity(peer.pid)
                        .is_some_and(|(now, label)| *created == now && *name == label),
                    Owner::Listed(name) => current_names
                        .get(&peer.pid)
                        .is_some_and(|label| label.eq_ignore_ascii_case(name)),
                    Owner::Unknown => false,
                })
                .into_iter()
                .flatten()
                .take(5)
                .cloned()
                .collect();
            ObservedPeer {
                pid: peer.pid,
                app: owner.name().map(str::to_owned),
                name_source: match owner {
                    Owner::Verified(..) => "verified",
                    Owner::Listed(_) => "windows-list",
                    Owner::Unknown => "unknown",
                },
                services,
                address: peer.remote,
                possible_name: possible_name.map(str::to_owned),
                domain_source,
                operator: network_operator(peer.remote.ip()),
                purpose,
                seen,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.seen
            .cmp(&a.seen)
            .then(a.app.cmp(&b.app))
            .then(a.address.cmp(&b.address))
            .then(a.pid.cmp(&b.pid))
    });
    omitted |= rows.len() > 150;
    rows.truncate(150);
    let named_destinations = rows.iter().any(|row| row.possible_name.is_some());
    Ok(Observation {
        samples,
        seconds,
        incomplete,
        named_destinations,
        omitted,
        rows,
    })
}

pub(crate) fn process_names() -> BTreeMap<u32, String> {
    let mut names = BTreeMap::new();
    // Toolhelp enumerates process names locally without opening their handles.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return names;
    }
    let mut process = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = unsafe { Process32FirstW(snapshot, &mut process) };
    while found != 0 {
        let end = process
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(process.szExeFile.len());
        let name = String::from_utf16_lossy(&process.szExeFile[..end]);
        let name = name
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        names.insert(process.th32ProcessID, name);
        found = unsafe { Process32NextW(snapshot, &mut process) };
    }
    unsafe { CloseHandle(snapshot) };
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_remote_ipv4_and_ignores_loopback_and_listening_rows() {
        let mut bytes = vec![0; TCP_V4.offset + 3 * TCP_V4.size];
        bytes[..4].copy_from_slice(&3u32.to_ne_bytes());
        for (index, (address, state)) in [
            ([192, 0, 2, 5], MIB_TCP_STATE_ESTAB),
            ([127, 0, 0, 1], MIB_TCP_STATE_ESTAB),
            ([198, 51, 100, 2], MIB_TCP_STATE_LISTEN),
        ]
        .into_iter()
        .enumerate()
        {
            let start = TCP_V4.offset + index * TCP_V4.size;
            let row = &mut bytes[start..start + TCP_V4.size];
            for (offset, value) in [
                (TCP_V4.pid, 7u32.to_ne_bytes()),
                (TCP_V4.state.unwrap(), (state as u32).to_ne_bytes()),
                (
                    std::mem::offset_of!(MIB_TCPROW_OWNER_PID, dwRemoteAddr),
                    address,
                ),
                (
                    std::mem::offset_of!(MIB_TCPROW_OWNER_PID, dwRemotePort),
                    u32::from(443u16.to_be()).to_ne_bytes(),
                ),
            ] {
                row[offset..offset + 4].copy_from_slice(&value);
            }
        }
        let peers = peer_rows(&bytes, false).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].pid, 7);
        assert_eq!(peers[0].remote.to_string(), "192.0.2.5:443");
        assert!(peer_rows(&bytes[..bytes.len() - 1], false).is_err());
    }

    #[test]
    fn only_labels_supported_exact_names_and_domain_suffixes() {
        assert_eq!(
            possible_purpose("v10.events.data.microsoft.com"),
            Some("Possible Windows diagnostics")
        );
        assert_eq!(
            possible_purpose("example.dl.delivery.mp.microsoft.com"),
            Some("Possible Windows or Store updates")
        );
        assert_eq!(
            possible_purpose("v10.events.data.microsoft.com.example.net"),
            None
        );
        assert_eq!(possible_purpose("notupdate.microsoft.com"), None);
        assert_eq!(possible_purpose("telemetry.example.net"), None);
    }

    #[test]
    fn identifies_published_networks_for_both_address_families() {
        for (address, expected) in [
            ("140.82.114.25", Some("GitHub")),
            ("140.82.128.0", None),
            ("104.18.32.47", Some("Cloudflare")),
            ("::ffff:104.18.32.47", Some("Cloudflare")),
            ("2606:4700::1", Some("Cloudflare")),
            ("192.0.2.8", None),
        ] {
            assert_eq!(
                network_operator(address.parse().unwrap()),
                expected,
                "{address}"
            );
        }
    }

    #[test]
    fn matches_dns_to_the_verified_process_and_its_lifetime() {
        let peer = Peer {
            pid: 7,
            remote: "192.0.2.5:443".parse().unwrap(),
        };
        let mut context = NetworkContext::default();
        context
            .dns
            .insert(peer.remote.ip(), BTreeSet::from(["cache.example".into()]));
        let queries = context
            .app_dns
            .entry((peer.pid, peer.remote.ip()))
            .or_default();
        for (image, at, name) in [
            ("App.exe", 20, "app.example"),
            ("App.exe", 9, "previous.example"),
            ("App.exe", 31, "future.example"),
            ("Other.exe", 20, "other.example"),
        ] {
            queries.push(AppDns {
                image: image.into(),
                at,
                name: name.into(),
            });
        }

        let owner = Owner::Verified(10, "app.exe".into());
        assert_eq!(
            context.destination_name(peer, &owner, 30),
            Some(("app.example", "app-dns"))
        );
        assert_eq!(
            context.destination_name(peer, &owner, 31),
            Some(("cache.example", "windows-cache"))
        );
        assert_eq!(
            context.destination_name(peer, &Owner::Listed("app.exe".into()), 30),
            Some(("cache.example", "windows-cache"))
        );
    }

    #[test]
    #[ignore = "takes an eight-second observation of real Windows connections"]
    fn observes_live_windows_without_requiring_dns() {
        let observation = observe(false).expect("live TCP observation");
        assert_eq!(observation.samples, 5);
        assert!(observation.seconds >= 8);
        assert!(observation.rows.iter().all(|row| {
            !row.address.ip().is_loopback() && row.seen > 0 && row.seen <= observation.samples
        }));
        println!(
            "{} readings, {} destination rows, {} process-list names, {} rows with services, {} network operators, {} app DNS matches",
            observation.samples,
            observation.rows.len(),
            observation
                .rows
                .iter()
                .filter(|row| row.name_source == "windows-list")
                .count(),
            observation
                .rows
                .iter()
                .filter(|row| !row.services.is_empty())
                .count(),
            observation
                .rows
                .iter()
                .filter(|row| row.operator.is_some())
                .count(),
            observation
                .rows
                .iter()
                .filter(|row| row.domain_source == Some("app-dns"))
                .count(),
        );
    }

    #[test]
    fn counts_only_established_tcp_and_listeners_and_ignores_unowned_sockets() {
        let bytes: Vec<_> = [4, 7, 5, 7, 2, 7, 11, 0, 5]
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect();
        let layout = Layout {
            offset: 4,
            size: 8,
            pid: 0,
            state: Some(4),
        };
        let mut result = BTreeMap::new();
        count_table(&bytes, layout, &mut result).unwrap();
        assert_eq!(
            result[&7],
            Counts {
                established: 1,
                listeners: 1,
                udp: 0
            }
        );
        assert_eq!(result.len(), 1);
        assert!(count_table(&bytes[..bytes.len() - 1], layout, &mut result).is_err());
    }

    #[test]
    fn windows_tables_attribute_test_loopback_sockets_to_this_process() {
        use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_server, _) = listener.accept().unwrap();
        let _udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let report = snapshot().unwrap();
        let pid = std::process::id().to_string();
        let row = report
            .lines()
            .find_map(|line| {
                let mut cells = line.split_whitespace().rev();
                let udp = cells.next()?.parse::<usize>().ok()?;
                let listeners = cells.next()?.parse::<usize>().ok()?;
                let established = cells.next()?.parse::<usize>().ok()?;
                (cells.next()? == pid).then_some((established, listeners, udp))
            })
            .expect("the report includes sockets owned by the test process");
        assert!(row.0 >= 2 && row.1 >= 1 && row.2 >= 1);
        assert!(!report.contains("127.0.0.1"));
    }
}
