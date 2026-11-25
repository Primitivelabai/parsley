use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemDirectoryW, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

// Formatting flags from the Windows SDK's pdh.h.
const PDH_FMT_NOSCALE: u32 = 0x0000_1000;
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

pub fn snapshot(inventory: bool) -> Result<String, String> {
    let script = if inventory {
        include_str!("system_inventory.ps1")
    } else {
        include_str!("system_health.ps1")
    };
    run_script(script)
}

pub fn version_snapshot() -> Result<crate::versions::Snapshot, String> {
    let script = format!(
        "$ParsleyVersionSnapshot = $true\n{}",
        include_str!("system_inventory.ps1")
    );
    crate::versions::Snapshot::from_windows(&run_script(&script)?)
}

pub(crate) fn run_script(script: &str) -> Result<String, String> {
    let system = system_directory()?;
    let windows_powershell = system.join("WindowsPowerShell").join("v1.0");
    let executable = windows_powershell.join("powershell.exe");
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = Command::new(executable)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("PSModulePath", windows_powershell.join("Modules"))
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("Could not start local system collection. {error}"))?;
    if !output.status.success() {
        return Err("Windows could not complete the system snapshot.".into());
    }
    let result = String::from_utf8(output.stdout)
        .map_err(|_| "Windows returned a system snapshot with an invalid encoding.")?;
    if result.trim().is_empty() {
        return Err("Windows returned an empty system snapshot.".into());
    }
    Ok(result)
}

fn system_directory() -> Result<PathBuf, String> {
    let mut buffer = vec![0u16; 32768];
    // Windows writes at most the supplied length, including the terminator.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err("Could not locate the Windows system directory.".into());
    }
    Ok(OsString::from_wide(&buffer[..length as usize]).into())
}

pub fn watch_health(
    warn: bool,
    mut take: impl FnMut(crate::live::Batch) -> bool,
) -> Result<(), String> {
    let mut previous = counters()?;
    let mut alerts = HealthAlerts::default();
    let performance = PerformanceCounters::open();
    if let Some(performance) = &performance {
        performance.collect();
    }
    let mut previous_network = NetworkSample::read();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let current = counters()?;
        let percent = current.busy_since(previous);
        let memory = memory_usage()?;
        let at = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
        let mut sample = crate::capture::Sample {
            unix_ms: at.timestamp_millis(),
            cpu_percent: percent,
            ram_percent: (memory.1 > 0.0).then(|| 100.0 * memory.0 / memory.1),
            gpu_peak_percent: None,
            disk_mib_per_sec: None,
            network_kib_per_sec: None,
        };
        let cpu = percent.map_or("sampling".into(), |value| format!("{value:.0}%"));
        let mut line = format!(
            "{}  CPU {cpu}  RAM {:.1} / {:.1} GiB",
            at.format("%H:%M:%S UTC"),
            memory.0,
            memory.1
        );
        if let Some(performance) = &performance
            && performance.collect()
        {
            use std::fmt::Write;
            if let Some((read, write)) = performance.storage() {
                sample.disk_mib_per_sec = Some((read + write) / 1_048_576.0);
                let _ = write!(
                    line,
                    "  Disk {:.2} read / {:.2} write MiB/s",
                    read / 1_048_576.0,
                    write / 1_048_576.0
                );
            }
            let loads = performance.gpu();
            sample.gpu_peak_percent = loads.values().copied().reduce(f64::max);
            if !loads.is_empty() {
                let _ = write!(line, "  GPU busiest engines ");
                for (index, load) in loads.values().enumerate() {
                    if index != 0 {
                        line.push_str(", ");
                    }
                    let _ = write!(line, "{load:.0}%");
                }
            }
        }
        let network = NetworkSample::read();
        if let Some((received, sent)) = network
            .as_ref()
            .zip(previous_network.as_ref())
            .and_then(|(now, prior)| now.rate_since(prior))
        {
            sample.network_kib_per_sec = Some((received + sent) / 1024.0);
            use std::fmt::Write;
            let _ = write!(
                line,
                "  Physical network {:.1} received / {:.1} sent KiB/s",
                received / 1024.0,
                sent / 1024.0
            );
        }
        previous_network = network;
        let notices = if warn {
            alerts.check(percent, sample.ram_percent)
        } else {
            Vec::new()
        };
        if !take(crate::live::Batch {
            lines: vec![crate::live::Line {
                text: line,
                reference: None,
            }],
            notices,
            health: Some(sample),
        }) {
            return Ok(());
        }
        previous = current;
    }
}

#[derive(Default)]
struct HealthAlerts {
    cpu: u8,
    memory: u8,
}

impl HealthAlerts {
    fn check(&mut self, cpu: Option<f64>, memory: Option<f64>) -> Vec<String> {
        let mut notices = Vec::new();
        for (name, streak, value) in [
            ("CPU", &mut self.cpu, cpu),
            ("Physical memory", &mut self.memory, memory),
        ] {
            if value.is_some_and(|percent| percent >= 90.0) {
                *streak = streak.saturating_add(1);
                if *streak >= 10 {
                    notices.push(format!(
                        "{name} has been at or above 90% for at least ten samples."
                    ));
                }
            } else {
                *streak = 0;
            }
        }
        notices
    }
}

struct NetworkSample {
    at: Instant,
    interfaces: HashMap<u64, (u64, u64)>,
}

impl NetworkSample {
    fn read() -> Option<Self> {
        let mut table = std::ptr::null_mut::<MIB_IF_TABLE2>();
        // The API allocates a variable-length local interface table.
        if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
            return None;
        }
        let table = InterfaceTable(table);
        let at = Instant::now();
        // Use the Win32 layout's table offset to account for struct padding.
        let rows = unsafe {
            let first = std::ptr::addr_of!((*table.0).Table).cast::<MIB_IF_ROW2>();
            std::slice::from_raw_parts(first, (*table.0).NumEntries as usize)
        };
        let mut interfaces = HashMap::new();
        const HARDWARE_INTERFACE_FLAG: u8 = 1;
        for row in rows {
            if row.OperStatus == IfOperStatusUp
                && row.InterfaceAndOperStatusFlags._bitfield & HARDWARE_INTERFACE_FLAG != 0
            {
                // NET_LUID is an opaque Windows identifier. Its Value member is a u64.
                let id = unsafe { row.InterfaceLuid.Value };
                interfaces.insert(id, (row.InOctets, row.OutOctets));
            }
        }
        Some(Self { at, interfaces })
    }

    fn rate_since(&self, previous: &Self) -> Option<(f64, f64)> {
        let elapsed = self.at.checked_duration_since(previous.at)?.as_secs_f64();
        if elapsed == 0.0 {
            return None;
        }
        let mut totals = None;
        for (interface, &(incoming, outgoing)) in &self.interfaces {
            if let Some(&(prior_in, prior_out)) = previous.interfaces.get(interface)
                && let (Some(received), Some(sent)) = (
                    incoming.checked_sub(prior_in),
                    outgoing.checked_sub(prior_out),
                )
            {
                let (in_total, out_total) = totals.get_or_insert((0.0, 0.0));
                *in_total += received as f64 / elapsed;
                *out_total += sent as f64 / elapsed;
            }
        }
        totals
    }
}

struct InterfaceTable(*mut MIB_IF_TABLE2);

impl Drop for InterfaceTable {
    fn drop(&mut self) {
        // GetIfTable2 requires FreeMibTable for its allocated buffer.
        unsafe { FreeMibTable(self.0.cast()) };
    }
}

struct PerformanceCounters {
    query: PDH_HQUERY,
    read: Option<PDH_HCOUNTER>,
    write: Option<PDH_HCOUNTER>,
    gpu: Option<PDH_HCOUNTER>,
}

impl PerformanceCounters {
    fn new() -> Option<Self> {
        let mut counters = Self {
            query: std::ptr::null_mut(),
            read: None,
            write: None,
            gpu: None,
        };
        // A null source reads the current Windows machine. This object owns the query.
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut counters.query) } != 0 {
            return None;
        }
        Some(counters)
    }

    fn add(&self, path: *const u16) -> Option<PDH_HCOUNTER> {
        let mut counter = std::ptr::null_mut();
        // Paths use the invariant English counter names and only the local machine.
        let status = unsafe { PdhAddEnglishCounterW(self.query, path, 0, &mut counter) };
        (status == 0 && !counter.is_null()).then_some(counter)
    }

    fn open() -> Option<Self> {
        let mut counters = Self::new()?;
        counters.read = counters.add(windows_sys::w!(
            r"\PhysicalDisk(_Total)\Disk Read Bytes/sec"
        ));
        counters.write = counters.add(windows_sys::w!(
            r"\PhysicalDisk(_Total)\Disk Write Bytes/sec"
        ));
        counters.gpu = counters.add(windows_sys::w!(r"\GPU Engine(*)\Utilization Percentage"));
        Some(counters)
    }

    fn collect(&self) -> bool {
        // All counters remain attached to this local query through sampling.
        unsafe { PdhCollectQueryData(self.query) == 0 }
    }

    fn storage(&self) -> Option<(f64, f64)> {
        let format = |counter| {
            let mut value = PDH_FMT_COUNTERVALUE::default();
            if unsafe {
                PdhGetFormattedCounterValue(
                    counter,
                    PDH_FMT_DOUBLE,
                    std::ptr::null_mut(),
                    &mut value,
                )
            } != 0
                || !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA)
            {
                return None;
            }
            // PDH_FMT_DOUBLE selects the active union member.
            let measured = unsafe { value.Anonymous.doubleValue };
            (measured.is_finite() && measured >= 0.0).then_some(measured)
        };
        Some((format(self.read?)?, format(self.write?)?))
    }

    fn gpu(&self) -> BTreeMap<String, f64> {
        let values = self
            .gpu
            .and_then(|counter| Self::array(counter, PDH_FMT_DOUBLE));
        busiest_gpu_engines(values.into_iter().flatten())
    }

    fn array(counter: PDH_HCOUNTER, format: u32) -> Option<Vec<(String, f64)>> {
        let mut bytes = 0;
        let mut count = 0;
        // A null item buffer asks PDH for the bytes required by the wildcard array.
        let required = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                format,
                &mut bytes,
                &mut count,
                std::ptr::null_mut(),
            )
        };
        if required != PDH_MORE_DATA || bytes == 0 || bytes > 4 * 1024 * 1024 {
            return None;
        }
        let mut data = vec![0u64; (bytes as usize).div_ceil(size_of::<u64>())];
        let items = data.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        // The backing storage is aligned for the platform item layout and is at least bytes long.
        let status =
            unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, items) };
        let used = bytes as usize;
        if status != 0
            || used > data.len() * size_of::<u64>()
            || count as usize > used / size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()
        {
            return None;
        }
        // PDH populated count complete entries in this allocation.
        let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
        let values = items.iter().filter_map(|item| {
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) {
                return None;
            }
            let offset = item.szName.addr().checked_sub(data.as_ptr().addr())?;
            if !offset.is_multiple_of(2) || offset >= used {
                return None;
            }
            // The string pointer was checked against the populated PDH buffer.
            let units = unsafe {
                std::slice::from_raw_parts(
                    data.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
                    (used - offset) / 2,
                )
            };
            let end = units.iter().position(|unit| *unit == 0)?;
            let name = String::from_utf16(&units[..end]).ok()?;
            // PDH_FMT_DOUBLE selects this member for each item as well.
            Some((name, unsafe { item.FmtValue.Anonymous.doubleValue }))
        });
        Some(values.collect())
    }
}

pub(crate) struct ProcessMemory {
    query: PerformanceCounters,
    private_ram: PDH_HCOUNTER,
}

impl ProcessMemory {
    pub(crate) fn open() -> Option<Self> {
        let query = PerformanceCounters::new()?;
        let private_ram = query.add(windows_sys::w!(r"\Process(*)\Working Set - Private"))?;
        Some(Self { query, private_ram })
    }

    pub(crate) fn read(&self) -> Option<MemoryReading> {
        if !self.query.collect() {
            return None;
        }
        let (used_gib, total_gib) = memory_usage().ok()?;
        let processes = group_private_ram(PerformanceCounters::array(
            self.private_ram,
            PDH_FMT_DOUBLE | PDH_FMT_NOSCALE | PDH_FMT_NOCAP100,
        )?);
        Some(MemoryReading {
            at: Instant::now(),
            used_gib,
            total_gib,
            processes,
        })
    }
}

pub(crate) struct MemoryReading {
    at: Instant,
    used_gib: f64,
    total_gib: f64,
    processes: BTreeMap<String, u64>,
}

#[derive(Default)]
pub(crate) struct MemoryHistory {
    readings: VecDeque<MemoryReading>,
}

#[derive(serde::Serialize)]
pub(crate) struct MemorySummary {
    used_gib: f64,
    total_gib: f64,
    seconds: u64,
    high: bool,
    apps: Vec<MemoryApp>,
}

#[derive(serde::Serialize)]
struct MemoryApp {
    name: String,
    bytes: u64,
    average_bytes: Option<u64>,
}

impl MemoryHistory {
    pub(crate) fn push(&mut self, reading: MemoryReading) {
        self.readings
            .retain(|prior| reading.at.duration_since(prior.at) <= Duration::from_secs(60));
        self.readings.push_back(reading);
    }

    pub(crate) fn summary(&self) -> Option<MemorySummary> {
        let latest = self.readings.back()?;
        let count = self.readings.len() as u64;
        let enough_readings = count >= 3;
        let mut apps: Vec<_> = latest
            .processes
            .iter()
            .map(|(name, &bytes)| {
                let average_bytes = enough_readings.then(|| {
                    self.readings
                        .iter()
                        .map(|reading| reading.processes.get(name).copied().unwrap_or(0))
                        .fold(0u64, u64::saturating_add)
                        / count
                });
                MemoryApp {
                    name: name.clone(),
                    bytes,
                    average_bytes,
                }
            })
            .collect();
        apps.sort_by(|a, b| {
            b.average_bytes
                .unwrap_or(b.bytes)
                .cmp(&a.average_bytes.unwrap_or(a.bytes))
                .then(b.bytes.cmp(&a.bytes))
                .then(a.name.cmp(&b.name))
        });
        apps.truncate(8);

        Some(MemorySummary {
            used_gib: latest.used_gib,
            total_gib: latest.total_gib,
            seconds: latest
                .at
                .duration_since(self.readings.front()?.at)
                .as_secs(),
            high: self.readings.len() >= 5
                && self.readings.iter().rev().take(5).all(|reading| {
                    reading.total_gib > 0.0 && reading.used_gib / reading.total_gib >= 0.9
                }),
            apps,
        })
    }
}

fn group_private_ram(values: Vec<(String, f64)>) -> BTreeMap<String, u64> {
    let mut totals = BTreeMap::<String, u64>::new();
    for (instance, value) in values {
        let name = instance
            .rsplit_once('#')
            .filter(|(_, suffix)| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
            .map_or(instance.as_str(), |(name, _)| name);
        if name.is_empty()
            || name.eq_ignore_ascii_case("_Total")
            || name.eq_ignore_ascii_case("Idle")
            || !value.is_finite()
            || value <= 0.0
        {
            continue;
        }
        let total = totals.entry(name.to_ascii_lowercase()).or_default();
        *total = total.saturating_add(value as u64);
    }
    totals
}

impl Drop for PerformanceCounters {
    fn drop(&mut self) {
        if !self.query.is_null() {
            // The owner closes the query and both attached counters together.
            unsafe { PdhCloseQuery(self.query) };
        }
    }
}

fn busiest_gpu_engines(values: impl Iterator<Item = (String, f64)>) -> BTreeMap<String, f64> {
    let mut engines = HashMap::<String, f64>::new();
    for (instance, load) in values {
        // A Windows GPU engine appears once per process. The suffix identifies
        // the hardware adapter and engine regardless of which process used it.
        if let Some((_, engine)) = instance.split_once("_luid_")
            && load.is_finite()
            && load >= 0.0
        {
            *engines.entry(engine.into()).or_default() += load;
        }
    }
    let mut adapters = BTreeMap::<String, f64>::new();
    for (engine, load) in engines {
        if let Some((adapter, _)) = engine.split_once("_eng_") {
            let current = adapters.entry(adapter.into()).or_default();
            *current = current.max(load.min(100.0));
        }
    }
    adapters
}

#[derive(Clone, Copy)]
struct Counters {
    idle: u64,
    kernel: u64,
    user: u64,
}

impl Counters {
    fn busy_since(self, previous: Self) -> Option<f64> {
        let kernel = self.kernel.checked_sub(previous.kernel)?;
        let user = self.user.checked_sub(previous.user)?;
        let total = kernel.checked_add(user)?;
        let idle = self.idle.checked_sub(previous.idle)?;
        let busy = total.checked_sub(idle)?;
        (total != 0).then(|| 100.0 * busy as f64 / total as f64)
    }
}

fn counters() -> Result<Counters, String> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // These are separate, writable FILETIME values that outlive the call.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return Err("Windows did not return CPU counters.".into());
    }
    let ticks =
        |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
    Ok(Counters {
        idle: ticks(idle),
        kernel: ticks(kernel),
        user: ticks(user),
    })
}

fn memory_usage() -> Result<(f64, f64), String> {
    let mut memory = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // The structure has the required length and is exclusively borrowed for the call.
    if unsafe { GlobalMemoryStatusEx(&mut memory) } == 0 {
        return Err("Windows did not return physical memory counters.".into());
    }
    const BYTES_PER_GIB: f64 = 1_073_741_824.0;
    let used = memory.ullTotalPhys.saturating_sub(memory.ullAvailPhys);
    Ok((
        used as f64 / BYTES_PER_GIB,
        memory.ullTotalPhys as f64 / BYTES_PER_GIB,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_subtracts_idle_from_combined_kernel_and_user_time() {
        let previous = Counters {
            idle: 200,
            kernel: 220,
            user: 70,
        };
        let current = Counters {
            idle: 240,
            kernel: 270,
            user: 120,
        };
        assert_eq!(current.busy_since(previous), Some(60.0));
        assert_eq!(previous.busy_since(current), None);
        assert_eq!(previous.busy_since(previous), None);
    }

    #[test]
    fn network_ignores_new_and_reset_interfaces() {
        let start = Instant::now();
        let initial = NetworkSample {
            at: start,
            interfaces: HashMap::from([(1, (100, 500)), (2, (9000, 100))]),
        };
        let current = NetworkSample {
            at: start + Duration::from_secs(2),
            interfaces: HashMap::from([(1, (1100, 1500)), (2, (9, 10)), (3, (50_000, 90_000))]),
        };
        assert_eq!(current.rate_since(&initial), Some((500.0, 500.0)));
    }

    #[test]
    fn high_usage_alert_requires_a_sustained_streak() {
        let mut flags = HealthAlerts::default();
        for _ in 0..9 {
            assert!(flags.check(Some(93.0), Some(89.0)).is_empty());
        }
        assert_eq!(flags.check(Some(93.0), Some(89.0)).len(), 1);
        assert!(flags.check(Some(40.0), None).is_empty());
        assert!(flags.check(Some(93.0), None).is_empty());
    }

    #[test]
    fn gpu_load_combines_processes_on_one_engine_and_keeps_adapters_distinct() {
        let samples = [
            ("pid_1_luid_a_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_2_luid_a_phys_0_eng_0_engtype_3D", 35.0),
            ("pid_1_luid_a_phys_0_eng_1_engtype_Copy", 52.0),
            ("pid_1_luid_b_phys_0_eng_0_engtype_3D", 7.0),
        ];
        let load = busiest_gpu_engines(
            samples
                .into_iter()
                .map(|(name, value)| (name.into(), value)),
        );
        assert_eq!(load.values().copied().collect::<Vec<_>>(), vec![60.0, 7.0]);
    }

    #[test]
    fn snapshot_collectors_accept_synthetic_windows_provider_data() {
        let mock = r#"
function Get-CimInstance {
    param([string] $ClassName, [string] $Filter, [int] $OperationTimeoutSec)
    [pscustomobject]@{
        Name = 'Demo Device'; Caption = 'Fictional Windows'; Version = '10.0'; BuildNumber = '26100'
        OSArchitecture = '64-bit'; Manufacturer = 'Demo Vendor'; SMBIOSBIOSVersion = '1.0'
        Product = 'Demo Board'; NumberOfCores = 8; NumberOfLogicalProcessors = 16
        DriverVersion = '1.2'; Model = 'Demo Drive'; Size = 1TB; Capacity = 16GB
        FreePhysicalMemory = 7MB; TotalVisibleMemorySize = 16MB; PhysicalAdapter = $true
        PercentProcessorTime = 50; IDProcess = 320; WorkingSet = 42MB
        DiskReadBytesPersec = 3MB; DiskWriteBytesPersec = 2MB
        BytesReceivedPersec = 6KB; BytesSentPersec = 1KB; UtilizationPercentage = 35
        PNPClass = 'Display'; Status = 'OK'; NetEnabled = $true
    }
}
function Get-ItemProperty {
    param($Path, $ErrorAction, $ErrorVariable)
    [pscustomobject]@{ DisplayName = 'Demo Software'; DisplayVersion = '1.2'; Publisher = 'Demo Vendor' }
}
function Get-Service {
    [pscustomobject]@{ DisplayName = 'Demo Service'; Status = 'Running'; StartType = 'Automatic' }
}
function Get-PhysicalDisk {
    param($ErrorAction)
    [pscustomobject]@{ FriendlyName = 'Demo Disk'; HealthStatus = 'Healthy' }
}
function Get-StorageReliabilityCounter {
    param($PhysicalDisk, $ErrorAction)
    [pscustomobject]@{ Temperature = 28; Wear = 12; PowerOnHours = 173 }
}
"#;
        let health = run_script(&format!("{mock}\n{}", include_str!("system_health.ps1"))).unwrap();
        let inventory =
            run_script(&format!("{mock}\n{}", include_str!("system_inventory.ps1"))).unwrap();
        assert!(health.contains("Demo Device") && health.contains("Top processes"));
        assert!(inventory.contains("Demo Software") && inventory.contains("Demo Service"));
        assert!(inventory.contains("Demo Disk") && inventory.contains("PowerOnHours"));
        assert!(!health.contains("did not return"));
        assert!(!inventory.contains("did not return"));
        let structured = run_script(&format!(
            "{mock}\n$ParsleyVersionSnapshot = $true\n{}",
            include_str!("system_inventory.ps1")
        ))
        .unwrap();
        let versions = crate::versions::Snapshot::from_windows(&structured)
            .unwrap()
            .json()
            .unwrap();
        assert!(versions.contains("Demo Software"));
        assert!(versions.contains("26100"));
        assert!(!versions.contains("Demo Service"));
        assert!(!versions.contains("Demo Disk"));
    }
}
#[test]
fn private_ram_groups_worker_instances_and_skips_aggregate_rows() {
    let values = vec![
        ("Example".to_owned(), 100.0),
        ("example#2".to_owned(), 150.0),
        ("_Total".to_owned(), 250.0),
        ("Idle".to_owned(), 12.0),
    ];
    let grouped = group_private_ram(values);
    assert_eq!(grouped, BTreeMap::from([("example".to_owned(), 250)]));
}

#[test]
fn memory_history_ranks_sustained_use_and_expires_old_readings() {
    let now = Instant::now();
    let mut history = MemoryHistory::default();
    for index in 0..5 {
        let mut processes = BTreeMap::from([("steady".to_owned(), 2_000)]);
        if index == 4 {
            processes.insert("spike".to_owned(), 5_000);
        }
        history.push(MemoryReading {
            at: now + Duration::from_secs(index * 5),
            used_gib: 30.0,
            total_gib: 32.0,
            processes,
        });
        assert_eq!(history.summary().unwrap().high, index == 4);
    }
    let summary = history.summary().unwrap();
    assert_eq!(summary.seconds, 20);
    assert_eq!(summary.apps[0].name, "steady");
    assert_eq!(summary.apps[1].average_bytes, Some(1_000));
    history.push(MemoryReading {
        at: now + Duration::from_secs(90),
        used_gib: 25.0,
        total_gib: 32.0,
        processes: BTreeMap::from([("steady".to_owned(), 1_000)]),
    });
    let summary = history.summary().unwrap();
    assert_eq!(summary.seconds, 0);
    assert!(!summary.high);
    assert_eq!(summary.apps[0].average_bytes, None);
}

#[test]
#[ignore = "requires working Windows performance counters on the host"]
fn reads_real_private_ram_in_bytes() {
    let counter = ProcessMemory::open().expect("private working set counter");
    let reading = counter.read().expect("first memory snapshot");
    assert!(!reading.processes.is_empty());
    assert!(
        reading
            .processes
            .values()
            .any(|&bytes| bytes >= 1024 * 1024)
    );
    assert!(reading.total_gib > 0.0);
    assert!(reading.used_gib <= reading.total_gib);
}
