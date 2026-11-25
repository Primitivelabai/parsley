#[cfg(windows)]
mod background;
mod capture;
#[cfg(windows)]
mod connections;
mod crash_groups;
#[cfg(windows)]
pub mod desktop;
mod export;
mod incident_case;
mod incidents;
#[cfg(windows)]
pub mod interface;
#[cfg(windows)]
mod live;
#[cfg(windows)]
mod network;
mod report;
#[cfg(windows)]
mod saved;
#[cfg(windows)]
mod system;
mod versions;
#[cfg(windows)]
mod windows;

use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};

const USAGE: &str = "Parsley Rewind

parsley demo REPORT.html
parsley demo-case graphics|application|restart REPORT.html
parsley case-create CASE.json --at RFC3339 [--minutes N] [--evtx INPUT.evtx] [--debugger-report ANALYSIS.txt --debugger-at RFC3339 [--include-module]] [--bookmark SOURCE:CHANNEL:RECORD]
parsley case-demo CASE.json
parsley case-report CASE.json REPORT.html
parsley case-bookmark CASE.json NEW.json SOURCE:CHANNEL:RECORD [...]
parsley demo-compare REPORT.html
parsley scan REPORT.html [--at RFC3339 --minutes N] [--include-app-names]
parsley import INPUT.evtx REPORT.html [--at RFC3339 --minutes N] [--include-app-names]
parsley compare REPORT.html --at RFC3339 --baseline RFC3339 [--minutes N] [--from INPUT.evtx] [--health CAPTURE.json] [--baseline-health CAPTURE.json] [--include-app-names]
parsley health REPORT.html
parsley inventory REPORT.html
parsley inventory-save SNAPSHOT.json
parsley inventory-compare BEFORE.json AFTER.json REPORT.html
parsley demo-inventory-compare REPORT.html
parsley demo-health REPORT.html
parsley demo-inventory REPORT.html
parsley connections REPORT.html
parsley demo-connections REPORT.html
parsley bandwidth REPORT.html
parsley demo-bandwidth REPORT.html
parsley history REPORT.html [--days 7|30|90] [--health CAPTURE.json] [--versions BEFORE.json AFTER.json] [--include-app-names]
parsley demo-history REPORT.html [--days 7|30|90]
parsley support REPORT.html [--include-health] [--include-inventory]
parsley demo-support REPORT.html [--include-health] [--include-inventory]
parsley evidence CHANNEL RECORD_ID [--from INPUT.evtx] [--expected-at RFC3339]
parsley watch
parsley watch-health [--samples N [--capture CAPTURE.json]]
parsley start-recording FILE.jsonl [--minutes N]
parsley recording-status FILE.jsonl
parsley stop-recording FILE.jsonl

scan reads the last 24 hours by default. import reads the supplied file.
--at centres Rewind on a timestamp with a time zone, such as
2026-11-22T14:14:00+08:00. --minutes sets minutes on each side (default 10).
Parsley creates a new report file and leaves existing files alone.";

pub fn cli() -> std::process::ExitCode {
    #[cfg(windows)]
    if let Some(exit) = network::maybe_worker() {
        return exit;
    }
    #[cfg(windows)]
    if let Some(exit) = background::maybe_worker() {
        return exit;
    }
    let mut args = env::args_os().skip(1).peekable();
    #[cfg(windows)]
    if args
        .peek()
        .is_some_and(|arg| arg == "watch" || arg == "watch-health")
    {
        let health = args.next().is_some_and(|arg| arg == "watch-health");
        if let Err(error) = watch_cli(health, args) {
            eprintln!("Parsley: {error}");
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
    }
    match run(args) {
        Ok(message) => {
            println!("{message}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Parsley: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn watch_cli(
    health: bool,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), String> {
    let mut remaining = None;
    let mut capture_path = None;
    while let Some(option) = args.next() {
        match option.to_str() {
            Some("--samples") if health && remaining.is_none() => {
                remaining = Some(
                    args.next()
                        .and_then(|value| value.to_str()?.parse::<usize>().ok())
                        .filter(|count| (1..=capture::MAX_SAMPLES).contains(count))
                        .ok_or("--samples must be 1–3600.")?,
                );
            }
            Some("--capture") if health && capture_path.is_none() => {
                let path =
                    PathBuf::from(args.next().ok_or("Expected a .json health capture path.")?);
                if export::Format::from_path(&path)? != export::Format::Json {
                    return Err("Health captures use .json output.".into());
                }
                if path.exists() {
                    return Err("Choose a new file to save the health capture.".into());
                }
                capture_path = Some(path);
            }
            _ => {
                return Err(
                    "Use watch or watch-health [--samples N [--capture CAPTURE.json]].".into(),
                );
            }
        }
    }
    if capture_path.is_some() && remaining.is_none() {
        return Err("--capture requires a finite --samples count.".into());
    }
    let mut samples = Vec::new();
    let mut relay = |batch: live::Batch| {
        if capture_path.is_some()
            && let Some(sample) = batch.health
        {
            samples.push(sample);
        }
        for notice in batch.notices {
            eprintln!("{notice}");
        }
        for line in batch.lines {
            println!("{}", line.text);
        }
        if let Some(count) = remaining.as_mut() {
            *count -= 1;
            return *count > 0;
        }
        true
    };
    if health {
        system::watch_health(false, &mut relay)?;
    } else {
        live::watch(&mut relay)?;
    }
    if let Some(path) = capture_path {
        write_report(&path, &capture::Capture::from_samples(samples).json()?)?;
        println!("Saved health capture to {}", path.display());
    }
    Ok(())
}

fn run(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<String, String> {
    let command = args.next().unwrap_or_else(|| "--help".into());
    let command = command.to_str().ok_or("Invalid command.")?;
    if matches!(
        command,
        "case-create" | "case-demo" | "case-report" | "case-bookmark"
    ) {
        return incident_case::run(command, args);
    }
    if matches!(
        command,
        "start-recording" | "recording-status" | "stop-recording"
    ) {
        #[cfg(windows)]
        return background::action(command, args);
        #[cfg(not(windows))]
        return Err("Background health recording requires Windows.".into());
    }
    if command == "evidence" {
        return evidence(args);
    }
    if command == "demo-case" {
        let id = args.next().ok_or("Choose a fictional case.")?;
        let case = incidents::DemoCase::get(id.to_str().ok_or("Invalid case name.")?)?;
        let output = PathBuf::from(args.next().ok_or("Choose a new HTML report file.")?);
        if export::Format::from_path(&output)? != export::Format::Html || args.next().is_some() {
            return Err("Use demo-case graphics|application|restart REPORT.html.".into());
        }
        let session = report::Session {
            events: case.events(),
            window: None,
            notices: vec![case.summary.into(), format!("Next check: {}", case.next)],
        };
        let html = report::render_demo(&session, case);
        return save(
            &output,
            export::Format::Html,
            &html,
            &[("timeline", &session)],
        );
    }
    if matches!(
        command,
        "inventory-save" | "inventory-compare" | "demo-inventory-compare"
    ) {
        return inventory_versions(command, args);
    }
    if matches!(command, "support" | "demo-support") {
        return support(args, command == "demo-support");
    }
    let mut source = match command {
        "demo" | "demo-compare" | "scan" | "compare" | "health" | "inventory" | "demo-health"
        | "demo-inventory" | "history" | "demo-history" | "connections" | "demo-connections"
        | "bandwidth" | "demo-bandwidth" => None,
        "import" => Some(log_path(PathBuf::from(
            args.next().ok_or("Expected an .evtx input path.")?,
        ))?),
        "--help" | "-h" => {
            return Ok(USAGE.into());
        }
        _ => return Err(format!("Unknown command.\n\n{USAGE}")),
    };
    let demo = matches!(command, "demo" | "demo-compare" | "demo-history");
    let output = PathBuf::from(args.next().ok_or("Expected an output report path.")?);
    let format = export::Format::from_path(&output)?;
    if matches!(
        command,
        "health"
            | "inventory"
            | "demo-health"
            | "demo-inventory"
            | "connections"
            | "demo-connections"
            | "bandwidth"
            | "demo-bandwidth"
    ) {
        if format != export::Format::Html {
            return Err("System snapshots use .html output.".into());
        }
        if args.next().is_some() {
            return Err("A system snapshot only takes an output path.".into());
        }
        if command == "bandwidth" {
            if output.try_exists().map_err(|error| error.to_string())? {
                return Err("Choose a new report file. Existing files are kept.".into());
            }
            if let Some(folder) = output
                .parent()
                .filter(|folder| !folder.as_os_str().is_empty())
                && !folder.is_dir()
            {
                return Err("The report folder could not be found.".into());
            }
        }
        let (title, snapshot) = snapshot(command)?;
        let html = report::render_snapshot(title, &snapshot);
        write_report(&output, &html)?;
        return Ok(format!("Saved {title} to {}", output.display()));
    }
    let mut at = None;
    let mut baseline = None;
    let mut minutes = None;
    let mut days = None;
    let mut health_path = None;
    let mut baseline_health_path = None;
    let mut versions_paths: Option<(PathBuf, PathBuf)> = None;
    let mut include_names = demo;
    while let Some(option) = args.next() {
        if option == "--include-app-names" && !include_names {
            include_names = true;
            continue;
        }
        let value = args.next().ok_or("Expected a value after the option.")?;
        if option == "--from" && command == "compare" && source.is_none() {
            source = Some(log_path(PathBuf::from(value))?);
            continue;
        }
        if option == "--versions" && command == "history" && versions_paths.is_none() {
            let after = args
                .next()
                .ok_or("--versions needs a before and an after snapshot.")?;
            versions_paths = Some((PathBuf::from(value), PathBuf::from(after)));
            continue;
        }
        if option == "--health" && matches!(command, "compare" | "history") && health_path.is_none()
        {
            health_path = Some(PathBuf::from(value));
            continue;
        }
        if option == "--baseline-health" && command == "compare" && baseline_health_path.is_none() {
            baseline_health_path = Some(PathBuf::from(value));
            continue;
        }
        let value = value.to_str().ok_or("Invalid option value.")?;
        match option.to_str() {
            Some("--at") if at.is_none() && !demo => {
                at = Some(timestamp(value, "--at")?);
            }
            Some("--baseline") if baseline.is_none() && command == "compare" => {
                baseline = Some(timestamp(value, "--baseline")?);
            }
            Some("--minutes") if minutes.is_none() && !demo => {
                let value = value
                    .parse::<i64>()
                    .map_err(|_| "--minutes must be 1–180.")?;
                if !(1..=180).contains(&value) {
                    return Err("--minutes must be 1–180.".into());
                }
                minutes = Some(value);
            }
            Some("--days") if days.is_none() && matches!(command, "history" | "demo-history") => {
                let value = value
                    .parse::<i64>()
                    .map_err(|_| "--days must be 7, 30 or 90.")?;
                if !matches!(value, 7 | 30 | 90) {
                    return Err("--days must be 7, 30 or 90.".into());
                }
                days = Some(value);
            }
            _ => return Err(format!("Unknown or repeated option.\n\n{USAGE}")),
        }
    }
    if minutes.is_some() && at.is_none() {
        return Err("--minutes requires --at.".into());
    }
    let radius = TimeDelta::minutes(minutes.unwrap_or(10));
    let name = if demo {
        "Fictional Windows events"
    } else if source.is_some() {
        "Imported Windows event log"
    } else {
        "Local Windows event logs"
    };
    if command == "history" && at.is_some() {
        return Err("Use --days, with 7, 30 or 90, to set the history window.".into());
    }
    if matches!(command, "history" | "demo-history") {
        let (now, mut session) = history_session(demo, days.unwrap_or(30))?;
        session.events.retain(|event| event.kind.is_incident());
        let health = health_path
            .as_deref()
            .map(capture::Capture::read)
            .transpose()?;
        let versions = match versions_paths {
            Some((before, after)) => Some((
                versions::Snapshot::read(&before)?,
                versions::Snapshot::read(&after)?,
            )),
            None => None,
        };
        let clues = crash_groups::Clues {
            health: health.as_ref(),
            versions: versions.as_ref().map(|(before, after)| (before, after)),
        };
        let analysis = crash_groups::analyze(&session.events, now, include_names, clues);
        let html = report::render_history(name, &session, &analysis, include_names);
        return save(&output, format, &html, &[("history", &session)]);
    }

    if command == "compare" {
        let incident = window(at.ok_or("Compare requires --at.")?, radius)?;
        let baseline = window(baseline.ok_or("Compare requires --baseline.")?, radius)?;
        if incident.0 <= baseline.1 && baseline.0 <= incident.1 {
            return Err("Choose separate comparison windows that do not overlap.".into());
        }
        if (health_path.is_some() || baseline_health_path.is_some())
            && format != export::Format::Html
        {
            return Err("Include a health capture in an .html comparison.".into());
        }
        let capture = health_path
            .as_deref()
            .map(capture::Capture::read)
            .transpose()?;
        let baseline_capture = baseline_health_path
            .as_deref()
            .map(capture::Capture::read)
            .transpose()?;
        let incident = collect(source.as_deref(), Some(incident))?;
        let baseline = collect(source.as_deref(), Some(baseline))?;
        let html = report::render_comparison(
            name,
            &incident,
            &baseline,
            include_names,
            [
                capture.as_ref(),
                baseline_capture.as_ref().or(capture.as_ref()),
            ],
        );
        return save(
            &output,
            format,
            &html,
            &[("incident", &incident), ("baseline", &baseline)],
        );
    }
    if command == "demo-compare" {
        let mut events = incidents::demo();
        let mut baseline_update = events[0].clone();
        baseline_update.at -= TimeDelta::days(1);
        baseline_update.record = 101;
        let incident_centre = events[3].at;
        let baseline_centre = incident_centre - TimeDelta::days(1);
        let incident_window = window(incident_centre, radius)?;
        events.retain(|event| (incident_window.0..=incident_window.1).contains(&event.at));
        let incident = report::Session {
            window: Some(incident_window),
            events,
            notices: Vec::new(),
        };
        let baseline = report::Session {
            window: Some(window(baseline_centre, radius)?),
            events: vec![baseline_update],
            notices: Vec::new(),
        };
        let capture = capture::Capture::demo(incident_centre, baseline_centre);
        let html = report::render_comparison(
            name,
            &incident,
            &baseline,
            include_names,
            [Some(&capture); 2],
        );
        return save(
            &output,
            format,
            &html,
            &[("incident", &incident), ("baseline", &baseline)],
        );
    }

    let selected = if let Some(at) = at {
        Some(window(at, radius)?)
    } else if command == "scan" {
        let now = DateTime::<Utc>::from(SystemTime::now());
        Some((now - TimeDelta::hours(24), now))
    } else {
        None
    };
    let session = if demo {
        report::Session {
            window: None,
            events: incidents::demo(),
            notices: Vec::new(),
        }
    } else {
        collect(source.as_deref(), selected)?
    };
    let html = report::render(name, &session, include_names);
    save(&output, format, &html, &[("timeline", &session)])
}

fn log_path(path: PathBuf) -> Result<PathBuf, String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("evtx"))
    {
        return Err("Expected an .evtx input file.".into());
    }
    std::path::absolute(path).map_err(|error| error.to_string())
}

fn timestamp(value: &str, option: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|_| format!("Use an RFC 3339 timestamp with a time zone for {option}."))
}

fn window(at: DateTime<Utc>, radius: TimeDelta) -> Result<report::Window, String> {
    at.checked_sub_signed(radius)
        .zip(at.checked_add_signed(radius))
        .ok_or_else(|| "Time window is out of range.".into())
}

#[cfg(windows)]
fn collect(
    source: Option<&Path>,
    window: Option<report::Window>,
) -> Result<report::Session, String> {
    let (mut events, notices) = windows::collect(
        source,
        window.map(|(start, _)| start),
        window.map(|(_, end)| end),
    )?;
    events.sort_by_key(|event| (event.at, event.kind.channel(), event.record));
    Ok(report::Session {
        window,
        events,
        notices,
    })
}

#[cfg(not(windows))]
fn collect(_: Option<&Path>, _: Option<report::Window>) -> Result<report::Session, String> {
    Err("Local scans and .evtx imports require Windows. Try demo or demo-compare.".into())
}

fn write_report(output: &Path, html: &str) -> Result<(), String> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut draft = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Could not create report. {error}"))?;
    draft
        .write_all(html.as_bytes())
        .map_err(|error| format!("Could not write report. {error}"))?;
    draft
        .persist_noclobber(output)
        .map_err(|error| format!("Could not save the new report. {}", error.error))?;
    Ok(())
}

fn save(
    output: &Path,
    format: export::Format,
    html: &str,
    sessions: &[(&str, &report::Session)],
) -> Result<String, String> {
    let data;
    let contents = if format == export::Format::Html {
        html
    } else {
        data = export::render(format, sessions)?;
        &data
    };
    write_report(output, contents)?;
    let count: usize = sessions
        .iter()
        .map(|(_, session)| session.events.len())
        .sum();
    let mut status = format!("Saved {count} events to {}", output.display());
    for notice in sessions.iter().flat_map(|(_, session)| &session.notices) {
        status.push('\n');
        status.push_str(notice);
    }
    Ok(status)
}

fn snapshot(command: &str) -> Result<(&'static str, String), String> {
    match command {
        "demo-connections" => Ok(("Fictional network connections by process", "Process                       PID  Established TCP  TCP listeners  UDP endpoints\ndemo-browser.exe              540                12              0              2\ndemo-server.exe               890                 3              1              0".into())),
        #[cfg(windows)]
        "connections" => connections::snapshot().map(|text| ("Network connections by process", text)),
        #[cfg(windows)]
        "bandwidth" => network::collect().map(|text| ("Bandwidth by process", text)),
        #[cfg(windows)]
        "demo-bandwidth" => Ok(("Fictional bandwidth by process", network::demo())),
        "demo-health" => Ok(("Fictional system health", "CPU\nTotal utilisation: 37%\n\nMemory\nIn use: 9.2 GiB of 16 GiB\n\nDisk throughput\nRead: 1.3 MiB/s  Write: 420 KiB/s\n\nNetwork\nReceived: 850 KiB/s  Sent: 34 KiB/s\n\nGPU engines\nGraphics: 22%\n\nProcesses\ndemo-editor  CPU 12%  Working set 640 MiB".into())),
        "demo-inventory" => Ok(("Fictional device inventory", "Windows\nWindows 11, build 26100\n\nProcessor\nExample 8-core CPU\n\nDisplay adapters\nExample Graphics 8 GB\n\nStorage\nExample NVMe 1 TB\n\nInstalled software\nDemo Editor 1.3\n\nServices\nDemo Service (running)".into())),
        #[cfg(windows)]
        "health" => system::snapshot(false).map(|content| ("System health", content)),
        #[cfg(windows)]
        "inventory" => system::snapshot(true).map(|content| ("Device inventory", content)),
        _ => Err("System snapshots require Windows.".into()),
    }
}

fn evidence(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<String, String> {
    let channel = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("Enter System or Application.")?;
    if !matches!(channel.as_str(), "System" | "Application") {
        return Err("Select the System or Application channel.".into());
    }
    let record = args
        .next()
        .and_then(|value| value.to_str()?.parse::<u64>().ok())
        .filter(|record| *record != 0)
        .ok_or("Enter a positive Windows record number.")?;
    let mut file = None;
    let mut expected_at = None;
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or("Expected a value after the evidence option.")?;
        match option.to_str() {
            Some("--from") if file.is_none() => file = Some(log_path(PathBuf::from(value))?),
            Some("--expected-at") if expected_at.is_none() => {
                expected_at = Some(timestamp(
                    value.to_str().ok_or("Expected a valid time.")?,
                    "--expected-at",
                )?);
            }
            _ => {
                return Err(
                    "Use evidence CHANNEL RECORD_ID [--from INPUT.evtx] [--expected-at RFC3339]."
                        .into(),
                );
            }
        }
    }
    #[cfg(windows)]
    return windows::evidence(&channel, record, file.as_deref(), expected_at);
    #[cfg(not(windows))]
    {
        let _ = (channel, record, file, expected_at);
        Err("Raw evidence inspection requires Windows.".into())
    }
}

fn support(
    mut args: impl Iterator<Item = std::ffi::OsString>,
    demo: bool,
) -> Result<String, String> {
    let output = PathBuf::from(args.next().ok_or("Expected an HTML support report path.")?);
    if export::Format::from_path(&output)? != export::Format::Html {
        return Err("The support report uses .html output.".into());
    }
    let (mut health, mut inventory) = (false, false);
    for flag in args {
        match flag.to_str() {
            Some("--include-health") if !health => health = true,
            Some("--include-inventory") if !inventory => inventory = true,
            _ => {
                return Err(
                    "Use support REPORT.html [--include-health] [--include-inventory].".into(),
                );
            }
        }
    }
    let session = if demo {
        report::Session {
            window: None,
            events: incidents::demo(),
            notices: Vec::new(),
        }
    } else {
        let now = DateTime::<Utc>::from(SystemTime::now());
        let window = Some((now - TimeDelta::hours(24), now));
        collect(None, window).unwrap_or_else(|error| report::Session {
            window,
            events: Vec::new(),
            notices: vec![error],
        })
    };
    let mut details = Vec::new();
    for (selected, live, fictional) in [
        (health, "health", "demo-health"),
        (inventory, "inventory", "demo-inventory"),
    ] {
        if selected {
            let snapshot = snapshot(if demo { fictional } else { live });
            details.push(snapshot.unwrap_or_else(|error| {
                let label = if live == "health" {
                    "System health"
                } else {
                    "Device inventory"
                };
                (label, error)
            }));
        }
    }
    let html = report::render_support(&session, &details, demo);
    write_report(&output, &html)?;
    Ok(format!("Saved support report to {}", output.display()))
}

fn inventory_versions(
    command: &str,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<String, String> {
    if command == "inventory-save" {
        let path = PathBuf::from(args.next().ok_or("Choose a .json version snapshot path.")?);
        if export::Format::from_path(&path)? != export::Format::Json || args.next().is_some() {
            return Err("Use inventory-save SNAPSHOT.json.".into());
        }
        #[cfg(windows)]
        {
            write_report(&path, &system::version_snapshot()?.json()?)?;
            return Ok(format!("Saved version snapshot to {}", path.display()));
        }
        #[cfg(not(windows))]
        return Err("Collecting Windows version information requires Windows.".into());
    }
    let demo = command == "demo-inventory-compare";
    let paths = if demo {
        None
    } else {
        Some((
            PathBuf::from(args.next().ok_or("Choose the earlier snapshot.")?),
            PathBuf::from(args.next().ok_or("Choose the later snapshot.")?),
        ))
    };
    let output = PathBuf::from(args.next().ok_or("Choose an .html comparison path.")?);
    if export::Format::from_path(&output)? != export::Format::Html || args.next().is_some() {
        return Err("Inventory comparisons use one .html output file.".into());
    }
    let (before, after) = match paths {
        Some((before, after)) => (
            versions::Snapshot::read(&before)?,
            versions::Snapshot::read(&after)?,
        ),
        None => versions::Snapshot::demo(),
    };
    let html = report::render_inventory_comparison(&before, &after, demo);
    write_report(&output, &html)?;
    Ok(format!(
        "Saved inventory comparison to {}",
        output.display()
    ))
}

/// Crash history for the chosen window and the time its frequencies end on.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn history_session(
    demo: bool,
    days: i64,
) -> Result<(DateTime<Utc>, report::Session), String> {
    if demo {
        let now = crash_groups::demo_now();
        let events = crash_groups::demo_events()
            .into_iter()
            .filter(|event| now - event.at <= TimeDelta::days(days))
            .collect();
        return Ok((
            now,
            report::Session {
                events,
                notices: Vec::new(),
                window: Some((now - TimeDelta::days(days), now)),
            },
        ));
    }
    let now = DateTime::<Utc>::from(SystemTime::now());
    Ok((
        now,
        collect(None, Some((now - TimeDelta::days(days), now)))?,
    ))
}
