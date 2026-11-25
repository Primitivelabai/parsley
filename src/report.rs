use std::fmt::Write;

use chrono::{DateTime, Utc};

use crate::capture::{Capture, METRICS};
use crate::incidents::{DemoCase, Detail, Event, HelpArticle, Kind};

pub type Window = (DateTime<Utc>, DateTime<Utc>);

pub struct Session {
    pub window: Option<Window>,
    pub events: Vec<Event>,
    pub notices: Vec<String>,
}

const HEAD: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'">
"#;

/// Incidents per day as a row of bars, oldest first, with the counts in text
/// for screen readers and print.
fn daily_strip(days: &[usize]) -> String {
    let peak = days.iter().copied().max().unwrap_or(0).max(1);
    let total: usize = days.iter().sum();
    let active = days.iter().filter(|count| **count > 0).count();
    let mut html = format!(
        "<figure class=\"daily\"><figcaption>Daily incidents, last {} days · {total} on {active} {}</figcaption><div class=\"bars\" aria-hidden=\"true\">",
        days.len(),
        if active == 1 { "day" } else { "days" },
    );
    for count in days {
        let _ = write!(
            html,
            "<span{} style=\"height:{}%\" title=\"{count}\"></span>",
            if *count > 0 { " class=\"on\"" } else { "" },
            if *count == 0 {
                8
            } else {
                25 + count * 75 / peak
            },
        );
    }
    let counts: Vec<String> = days.iter().map(ToString::to_string).collect();
    let _ = write!(
        html,
        "</div><p class=\"counts\">{}</p></figure>",
        counts.join(" ")
    );
    html
}

/// The pixel sprig, drawn inline so reports need no image files.
const SPRIG: &str = r##"<svg viewBox="0 0 16 16" aria-hidden="true"><path fill="#38a04a" d="M5 0h2v1h-2zM9 0h2v1h-2zM4 1h8v1h-8zM3 2h4v1h-4zM9 2h4v1h-4zM4 3h3v1h-3zM9 3h3v1h-3zM1 4h2v1h-2zM6 4h2v1h-2zM10 4h1v1h-1zM13 4h2v1h-2zM0 5h4v1h-4zM7 5h1v1h-1zM9 5h1v1h-1zM12 5h4v1h-4zM0 6h2v1h-2zM3 6h2v1h-2zM11 6h2v1h-2zM14 6h2v1h-2zM1 7h2v1h-2zM4 7h2v1h-2zM10 7h2v1h-2zM13 7h2v1h-2zM2 8h3v1h-3zM11 8h3v1h-3zM3 9h3v1h-3zM10 9h2v1h-2z"/><path fill="#8fd47f" d="M7 2h2v1h-2zM7 3h2v1h-2zM8 4h2v1h-2zM2 6h1v1h-1zM13 6h1v1h-1zM3 7h1v1h-1zM12 7h1v1h-1zM5 8h1v1h-1zM10 8h1v1h-1z"/><path fill="#2b7d39" d="M8 5h1v1h-1zM8 6h1v1h-1zM8 7h1v1h-1zM6 8h1v1h-1zM8 8h1v1h-1zM6 9h4v1h-4zM7 10h2v1h-2zM7 11h1v1h-1zM6 12h2v1h-2zM6 13h1v1h-1zM5 14h2v1h-2z"/></svg>"##;

fn report_start(title: &str) -> String {
    let title = escape(title);
    format!(
        "{HEAD}<title>{title} · Parsley</title><style>{}{}</style></head>\n<body><main>\n<header><p class=\"brand\">{SPRIG}Parsley<span class=\"kind\">Local report</span></p><h1>{title}</h1>\n",
        include_str!("report-fonts.css"),
        include_str!("report.css")
    )
}

const FOOT: &str = r#"
<footer>Reports include event types, timestamps and Windows record references. Event payloads and computer and user fields are omitted.</footer>
</main>
<script>
const printButton = document.querySelector('#print-report')
if (printButton) printButton.addEventListener('click', () => window.print())
const filter = document.querySelector('#category')
const search = document.querySelector('#query')
const status = document.querySelector('#results')
const events = document.querySelectorAll('article[data-kind]')
if (filter && search && status) {
  function applyFilters() {
    let visible = 0
    const query = search.value.trim().toLowerCase()
    for (const event of events) {
      event.hidden = (Boolean(filter.value) && event.dataset.kind !== filter.value) ||
        (Boolean(query) && !event.textContent.toLowerCase().includes(query))
      if (!event.hidden) visible += 1
    }
    status.textContent = visible === 1 ? '1 event shown' : visible + ' events shown'
  }
  filter.addEventListener('change', applyFilters)
  search.addEventListener('input', applyFilters)
  document.querySelector('[data-evidence-link]')?.addEventListener('click', event => {
    const target = document.getElementById(event.currentTarget.hash.slice(1))
    if (!target) return
    if (target.hidden) {
      filter.value = ''
      search.value = ''
      applyFilters()
    }
    const evidence = target.querySelector('.evidence')
    if (evidence) evidence.open = true
  })
}
</script>
</body></html>
"#;

pub fn render(source: &str, session: &Session, include_names: bool) -> String {
    render_timeline(source, session, include_names, None)
}

pub fn render_demo(session: &Session, case: &DemoCase) -> String {
    render_timeline("Example report · Made-up data", session, true, Some(case))
}

fn render_timeline(
    source: &str,
    session: &Session,
    include_names: bool,
    example: Option<&DemoCase>,
) -> String {
    let mut html = report_start(example.map_or("Windows event report", |case| case.title));
    let _ = writeln!(html, "<p>{}</p>", escape(source));
    html.push_str(PRINT_BUTTON);
    write_window(&mut html, session.window);
    html.push_str("</header>\n");
    if let Some(case) = example {
        let _ = writeln!(
            html,
            "<section aria-labelledby=\"overview\"><h2 id=\"overview\">What happened</h2><p>{}</p><div class=\"next-check\"><h3>Next check</h3><p>{}</p>",
            escape(case.summary),
            escape(case.next)
        );
        if let Some(event) = session
            .events
            .iter()
            .find(|event| event.kind == case.evidence)
        {
            let _ = writeln!(
                html,
                "<a href=\"#{}-{}\" data-evidence-link>Show the evidence</a>",
                event.kind.channel(),
                event.record
            );
        }
        html.push_str("</div></section>\n");
    }
    html.push_str(
        "<section aria-labelledby=\"timeline\"><h2 id=\"timeline\">Windows event timeline</h2>\n",
    );
    if example.is_none() {
        write_notices(&mut html, session);
        write_names_notice(&mut html, include_names);
    }
    if !session.events.is_empty() {
        let _ = writeln!(
            html,
            "<div class=\"filters\"><label>Search events<input id=\"query\" type=\"search\"></label><label>Event type<select id=\"category\"><option value=\"\">All events</option><option value=\"restart\">Restarts</option><option value=\"app\">App crashes and hangs</option><option value=\"graphics\">Graphics</option><option value=\"update\">Updates</option><option value=\"service\">Services</option><option value=\"hardware\">Hardware</option><option value=\"power\">Sleep and wake</option></select></label><p id=\"results\" role=\"status\">{} {}</p></div>",
            session.events.len(),
            if session.events.len() == 1 {
                "event shown"
            } else {
                "events shown"
            }
        );
    }
    write_events(
        &mut html,
        &session.events,
        "",
        include_names,
        None,
        example.is_none(),
    );
    html.push_str("</section>");
    html.push_str(FOOT);
    html
}

pub fn render_case(case: &crate::incident_case::Case) -> Result<String, String> {
    use crate::incident_case::{Channel, Correlation, RecordRef};

    let mut html = report_start("Incident case report");
    let label = if case.fictional() {
        "Fictional incident case"
    } else {
        "Redacted incident case"
    };
    let _ = writeln!(html, "<p>{label}</p>");
    html.push_str(PRINT_BUTTON);
    write_window(&mut html, Some(case.window()?));
    html.push_str("</header>");
    if let Some(debugger) = &case.debugger {
        html.push_str(
            "<section aria-labelledby=\"debugger\"><h2 id=\"debugger\">Debugger evidence</h2>",
        );
        let at = crate::timestamp(&debugger.at_utc, "debugger crash time")?;
        let _ = writeln!(
            html,
            "<p>Imported stop code <code>0x{:08X}</code> · {}.</p>",
            debugger.stop_code,
            at.format("%Y-%m-%d %H:%M:%S UTC")
        );
        if let Some(name) = Detail::stop_name(debugger.stop_code) {
            let _ = writeln!(html, "<p>Windows stop name <code>{name}</code>.</p>");
        }
        if let Some(module) = &debugger.module {
            let _ = writeln!(
                html,
                "<p>Module reported by the debugger <code>{}</code>.</p>",
                escape(module)
            );
        }
        match case.correlation(debugger)? {
            Correlation::Unmatched => html.push_str("<p>No Windows stop record had the same code from three minutes before to 15 minutes after the crash.</p>"),
            Correlation::Ambiguous => html.push_str("<p>Several Windows stop records had the same code from three minutes before to 15 minutes after the crash. The debugger report remains unlinked.</p>"),
            Correlation::Candidate(record) => {
                let _ = writeln!(html, "<p><a href=\"#{}\">Candidate Windows stop record</a> from source {} has the same code and was logged within 15 minutes of the crash. Confirm the two sources came from the same PC.</p>", record.anchor(), record.source);
                let events = case.sources[record.source - 1].records.iter()
                    .map(crate::incident_case::CaseEvent::event).collect::<Result<Vec<_>, _>>()?;
                if let Some(restart) = events.iter().find(|event| {
                    event.nearby_stop_error(&events) == Some(record.record)
                        && event.detail.stop_code.is_none_or(|code| code == debugger.stop_code)
                }) {
                    let restart = RecordRef { source: record.source, channel: Channel::System, record: restart.record };
                    let _ = writeln!(html, "<p>The same source logged a nearby <a href=\"#{}\">unexpected restart</a>.</p>", restart.anchor());
                }
            }
        }
        html.push_str("<p>A module name is a clue. Check the driver version and any repeated occurrences before drawing a conclusion.</p></section>");
    }
    if !case.bookmarks.is_empty() {
        html.push_str(
            "<section aria-labelledby=\"bookmarks\"><h2 id=\"bookmarks\">Bookmarks</h2><ul>",
        );
        for reference in &case.bookmarks {
            let _ = writeln!(
                html,
                "<li><a href=\"#{}\">Source {} · {} · Record {}</a></li>",
                reference.anchor(),
                reference.source,
                reference.channel.name(),
                reference.record
            );
        }
        html.push_str("</ul></section>");
    }
    let mut timeline = Vec::new();
    let mut by_source = Vec::new();
    for (index, source) in case.sources.iter().enumerate() {
        let number = index + 1;
        let mut events = Vec::new();
        for record in &source.records {
            let event = record.event()?;
            timeline.push((event.at, record.reference(number), event.kind));
            events.push(event);
        }
        events.sort_by_key(|event| (event.at, event.kind.channel(), event.record));
        by_source.push((number, source.partial, events));
    }
    timeline.sort_by_key(|(at, reference, _)| {
        (
            *at,
            reference.source,
            reference.channel.name(),
            reference.record,
        )
    });
    html.push_str("<section aria-labelledby=\"case-timeline\"><h2 id=\"case-timeline\">Incident timeline</h2>");
    if timeline.is_empty() {
        html.push_str("<p>No supported events were found in the case window.</p>");
    } else {
        html.push_str("<table><thead><tr><th scope=\"col\">Time</th><th scope=\"col\">Event</th><th scope=\"col\">Evidence</th></tr></thead><tbody>");
        for (at, reference, kind) in timeline {
            let _ = writeln!(
                html,
                "<tr><td>{}</td><td>{}</td><td><a href=\"#{}\">Source {} · {} · Record {}</a></td></tr>",
                at.format("%Y-%m-%d %H:%M:%S UTC"),
                kind.title(),
                reference.anchor(),
                reference.source,
                reference.channel.name(),
                reference.record
            );
        }
        html.push_str("</tbody></table>");
    }
    html.push_str("</section>");
    for (number, partial, events) in by_source {
        let _ = writeln!(
            html,
            "<section aria-labelledby=\"source-{number}\"><h2 id=\"source-{number}\">Source {number}</h2>"
        );
        if partial {
            html.push_str("<p>Record coverage was incomplete for this source.</p>");
        }
        write_events(
            &mut html,
            &events,
            &format!("source-{number}-"),
            false,
            None,
            !case.fictional(),
        );
        html.push_str("</section>");
    }
    html.push_str(FOOT);
    Ok(html)
}

pub fn render_comparison(
    source: &str,
    incident: &Session,
    baseline: &Session,
    include_names: bool,
    captures: [Option<&crate::capture::Capture>; 2],
) -> String {
    let mut html = report_start("Session comparison");
    let _ = writeln!(html, "<p>{}</p>{PRINT_BUTTON}</header>", escape(source));
    html.push_str(
        "<section aria-labelledby=\"comparison\"><h2 id=\"comparison\">What changed</h2>\n\
        <p>Counts show what Windows recorded in each window. Timing alone cannot establish cause.</p>\n",
    );
    if !incident.notices.is_empty() || !baseline.notices.is_empty() {
        html.push_str(
            "<p><strong>Some logs are incomplete. Treat these counts as partial.</strong></p>\n",
        );
    }
    write_names_notice(&mut html, include_names);
    if incident.events.is_empty() && baseline.events.is_empty() {
        html.push_str("<p>Neither window contained supported event records.</p>\n");
    } else {
        html.push_str("<table><thead><tr><th scope=\"col\">Recorded event</th><th scope=\"col\">Incident</th><th scope=\"col\">Baseline</th><th scope=\"col\">Difference</th></tr></thead><tbody>\n");
        for kind in Kind::ALL {
            let incident_count = incident
                .events
                .iter()
                .filter(|event| event.kind == kind)
                .count();
            let baseline_count = baseline
                .events
                .iter()
                .filter(|event| event.kind == kind)
                .count();
            if incident_count == 0 && baseline_count == 0 {
                continue;
            }
            let incident_cell = count_cell(incident, kind, incident_count, "incident-");
            let baseline_cell = count_cell(baseline, kind, baseline_count, "baseline-");
            let difference = incident_count as i64 - baseline_count as i64;
            let _ = writeln!(
                html,
                "<tr><th scope=\"row\">{}</th>{incident_cell}{baseline_cell}<td>{}</td></tr>",
                kind.title(),
                if difference == 0 {
                    "0".into()
                } else {
                    format!("{difference:+}")
                },
            );
        }
        html.push_str("</tbody></table>\n");
    }
    html.push_str("</section>\n");
    if captures.iter().any(Option::is_some)
        && let Some((incident_window, baseline_window)) = incident.window.zip(baseline.window)
    {
        write_health_comparison(&mut html, captures, incident_window, baseline_window);
    }
    for (name, prefix, session, health) in [
        ("Incident window", "incident-", incident, captures[0]),
        ("Baseline window", "baseline-", baseline, captures[1]),
    ] {
        let _ = writeln!(html, "<section aria-label=\"{name}\"><h2>{name}</h2>");
        write_window(&mut html, session.window);
        write_notices(&mut html, session);
        write_events(
            &mut html,
            &session.events,
            prefix,
            include_names,
            health.zip(session.window),
            true,
        );
        html.push_str("</section>\n");
    }
    html.push_str(FOOT);
    html
}

fn write_health_comparison(
    html: &mut String,
    captures: [Option<&crate::capture::Capture>; 2],
    incident: Window,
    baseline: Window,
) {
    let summary = |capture: Option<&crate::capture::Capture>, window| {
        capture.map_or_else(crate::capture::Summary::default, |capture| {
            capture.summarize(window)
        })
    };
    let incident = summary(captures[0], incident);
    let baseline = summary(captures[1], baseline);
    html.push_str("<section aria-labelledby=\"health-comparison\"><h2 id=\"health-comparison\">Captured system health</h2><p>These are peaks from the supplied samples. Event details show a nearby sample when one was captured within ten seconds. A timestamp match alone cannot establish cause.</p>");
    for (label, summary) in [("Incident", &incident), ("Baseline", &baseline)] {
        if let Some((first, last)) = summary.coverage {
            let _ = writeln!(
                html,
                "<p>{label} had {} samples, from {} to {} UTC.</p>",
                summary.samples,
                first.format("%Y-%m-%d %H:%M:%S"),
                last.format("%Y-%m-%d %H:%M:%S"),
            );
        } else {
            let _ = writeln!(
                html,
                "<p>No health samples covered the {} window.</p>",
                label.to_lowercase()
            );
        }
    }
    if incident.samples + baseline.samples > 0 {
        html.push_str("<table><thead><tr><th scope=\"col\">Peak sampled</th><th scope=\"col\">Incident</th><th scope=\"col\">Baseline</th></tr></thead><tbody>");
        for ((label, unit), (inc, base)) in crate::capture::METRICS
            .into_iter()
            .zip(incident.peak.into_iter().zip(baseline.peak))
        {
            let format = |value: Option<f64>| {
                value.map_or_else(|| "Unavailable".into(), |value| format!("{value:.1}{unit}"))
            };
            let _ = writeln!(
                html,
                "<tr><th scope=\"row\">{label}</th><td>{}</td><td>{}</td></tr>",
                format(inc),
                format(base)
            );
        }
        html.push_str("</tbody></table>");
    }
    html.push_str("</section>");
}

fn count_cell(session: &Session, kind: Kind, count: usize, prefix: &str) -> String {
    match session.events.iter().find(|event| event.kind == kind) {
        Some(event) => format!(
            "<td><a href=\"#{prefix}{}-{}\">{count}</a></td>",
            kind.channel(),
            event.record,
        ),
        None => "<td>0</td>".into(),
    }
}

pub fn render_history(
    source: &str,
    session: &Session,
    analysis: &crate::crash_groups::Analysis,
    include_names: bool,
) -> String {
    let mut html = report_start("Crash history");
    let _ = writeln!(html, "<p>{}</p>", escape(source));
    html.push_str(PRINT_BUTTON);
    write_window(&mut html, session.window);
    html.push_str(
        "</header><section aria-labelledby=\"history\"><h2 id=\"history\">Recurring incidents</h2>",
    );
    write_notices(&mut html, session);
    write_names_notice(&mut html, include_names);
    for notice in &analysis.notices {
        let _ = writeln!(html, "<p>{}</p>", escape(notice));
    }
    let _ = writeln!(
        html,
        "<p data-incidents=\"{0}\">{0} incidents in {1} groups. A linked stop error and restart count as one incident.</p>",
        analysis.incidents,
        analysis.groups.len()
    );
    if !analysis.groups.is_empty() {
        html.push_str("<table><thead><tr><th scope=\"col\">Recurring incident</th><th scope=\"col\">Incidents</th><th scope=\"col\">First</th><th scope=\"col\">Last</th><th scope=\"col\">7 days</th><th scope=\"col\">30 days</th><th scope=\"col\">90 days</th></tr></thead><tbody>");
        for group in &analysis.groups {
            let sum = |days: &[usize]| days.iter().sum::<usize>();
            let _ = writeln!(
                html,
                "<tr><th scope=\"row\"><a href=\"#{}\">{}</a></th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                group.key,
                escape(&group.title),
                group.incidents,
                short_time(&group.first),
                short_time(&group.last),
                sum(&group.frequency.days7),
                sum(&group.frequency.days30),
                sum(&group.frequency.days90),
            );
        }
        html.push_str("</tbody></table>");
    }
    for group in &analysis.groups {
        let _ = writeln!(
            html,
            "<article id=\"{}\" data-group=\"{}\" data-incidents=\"{}\"><h3>{}</h3><p>{}</p><h4>Evidence</h4><ul>",
            group.key,
            group.basis,
            group.incidents,
            escape(&group.title),
            escape(&group.explanation),
        );
        for line in &group.evidence {
            let _ = writeln!(html, "<li>{}</li>", escape(line));
        }
        html.push_str("</ul>");
        if !group.clues.is_empty() {
            html.push_str("<h4>Around the incident</h4><ul>");
            for line in &group.clues {
                let _ = writeln!(html, "<li>{}</li>", escape(line));
            }
            html.push_str("</ul>");
        }
        write_next_step(&mut html, &group.next_check, group.help);
        let _ = writeln!(
            html,
            "{}<h4>Windows records</h4><ul>",
            daily_strip(&group.frequency.days30)
        );
        for record in &group.records {
            let linked = if record.linked.is_some() {
                " · linked pair"
            } else {
                ""
            };
            let _ = writeln!(
                html,
                "<li><a href=\"#{}-{}\">{} · {} · Record {}</a>{linked}</li>",
                record.channel,
                record.record,
                short_time(&record.at),
                record.title,
                record.record,
            );
        }
        html.push_str("</ul></article>");
    }
    html.push_str("<h2>All records</h2>");
    write_events(&mut html, &session.events, "", include_names, None, true);
    html.push_str("</section>");
    html.push_str(FOOT);
    html
}

fn short_time(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| escape(value))
}

fn write_window(html: &mut String, window: Option<Window>) {
    if let Some((start, end)) = window {
        let _ = writeln!(
            html,
            "<p>{} to {} UTC</p>",
            start.format("%Y-%m-%d %H:%M:%S"),
            end.format("%Y-%m-%d %H:%M:%S"),
        );
    }
}

fn write_notices(html: &mut String, session: &Session) {
    for notice in &session.notices {
        let _ = writeln!(html, "<p>{}</p>", escape(notice));
    }
}

fn write_names_notice(html: &mut String, include_names: bool) {
    if include_names {
        html.push_str(
            "<p>This report includes application and module names. Review it before sharing.</p>\n",
        );
    }
}

fn write_next_step(html: &mut String, step: &str, article: Option<HelpArticle>) {
    let _ = writeln!(
        html,
        "<div class=\"event-next\"><p><strong>What can I do next?</strong></p><p>{}</p>",
        escape(step)
    );
    if let Some(article) = article {
        let _ = writeln!(
            html,
            "<a class=\"official-help\" href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">{}</a>",
            article.url, article.label
        );
    }
    html.push_str("</div>\n");
}

fn write_events(
    html: &mut String,
    events: &[Event],
    prefix: &str,
    include_names: bool,
    health: Option<(&Capture, Window)>,
    guidance: bool,
) {
    if events.is_empty() {
        html.push_str(
            "<p>No supported events were found in the selected logs and time range.</p>\n",
        );
    }
    for event in events {
        let at = event.at.to_rfc3339();
        let kind = event.kind;
        let detail = &event.detail;
        let _ = writeln!(
            html,
            "<article id=\"{prefix}{}-{}\" data-kind=\"{}\"><time datetime=\"{}\">{}</time><h3>{}</h3><p>{}</p>",
            kind.channel(),
            event.record,
            kind.key(),
            at,
            event.at.format("%Y-%m-%d %H:%M:%S UTC"),
            kind.title(),
            kind.explanation(),
        );
        if guidance {
            write_next_step(html, kind.next_step(), kind.help());
        }
        if kind.is_incident()
            && let Some((capture, window)) = health
            && let Some((sample, gap)) = capture.nearest(event.at, window)
        {
            let when = match gap {
                0 => "at the event timestamp".into(),
                gap if gap < 0 => format!(
                    "{:.1} s before the event",
                    gap.unsigned_abs() as f64 / 1000.0
                ),
                gap => format!("{:.1} s after the event", gap as f64 / 1000.0),
            };
            let readings: Vec<_> = METRICS
                .into_iter()
                .zip(sample.metrics())
                .filter_map(|((label, unit), value)| {
                    value.map(|value| format!("{label} {value:.1}{unit}"))
                })
                .collect();
            if !readings.is_empty() {
                let _ = writeln!(
                    html,
                    "<details><summary>Health sample {when}</summary><p>{}</p></details>",
                    readings.join(" · ")
                );
            }
        }
        if let Some(code) = detail.stop_code {
            let _ = write!(html, "<p>Stop code <code>0x{code:08X}</code>");
            if let Some(name) = Detail::stop_name(code) {
                let _ = write!(html, " · <code>{name}</code>");
            }
            html.push_str("</p>\n");
        }
        if include_names {
            if let Some(application) = &detail.application {
                let _ = writeln!(
                    html,
                    "<p>Application <code>{}</code></p>",
                    escape(application)
                );
            }
            if let Some(module) = &detail.module {
                let _ = writeln!(
                    html,
                    "<p>Faulting module <code>{}</code></p>",
                    escape(module)
                );
            }
        }
        if let Some(exception) = detail.exception {
            let _ = writeln!(html, "<p>Exception code <code>0x{exception:08X}</code></p>");
        }
        if let Some(record) = event.nearby_stop_error(events) {
            let _ = writeln!(
                html,
                "<p>Windows logged a <a href=\"#{prefix}System-{record}\">stop error</a> shortly after this restart.</p>"
            );
        }
        let _ = writeln!(
            html,
            "<details class=\"evidence\"><summary>Windows evidence</summary><p>{} · {} · Event ID {} · Record {}</p></details></article>",
            kind.channel(),
            kind.provider(),
            event.id,
            event.record,
        );
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

const PRINT_BUTTON: &str =
    "<button id=\"print-report\" type=\"button\">Print or save as PDF</button>";
const PRINT_FOOT: &str = "</main><script>document.querySelector('#print-report').addEventListener('click', () => window.print())</script></body></html>";

pub fn render_snapshot(title: &str, snapshot: &str) -> String {
    let mut html = report_start(title);
    let _ = writeln!(
        html,
        "<p>Review device and process details before sharing this report.</p>{PRINT_BUTTON}</header><section aria-labelledby=\"results-heading\"><h2 id=\"results-heading\">Results</h2><pre tabindex=\"0\">{}</pre></section>",
        escape(snapshot),
    );
    html.push_str(PRINT_FOOT);
    html
}

pub fn render_inventory_comparison(
    before: &crate::versions::Snapshot,
    after: &crate::versions::Snapshot,
    demo: bool,
) -> String {
    let mut html = report_start("Inventory version comparison");
    html.push_str(PRINT_BUTTON);
    html.push_str(
        "</header><section aria-label=\"Inventory version comparison\"><h2>Snapshots compared</h2>",
    );
    if demo {
        html.push_str("<p>Fictional version snapshots.</p>");
    }
    let _ = writeln!(
        html,
        "<p>Before {} · After {}</p>",
        escape(&before.time_utc),
        escape(&after.time_utc)
    );
    html.push_str("<p>Products are grouped by the names Windows reported. Review software and hardware names before sharing.</p></section>");
    for name in crate::versions::GROUPS {
        let _ = writeln!(html, "<section><h2>{name}</h2>");
        match before.changes(after, name) {
            None => html.push_str("<p>This category was unavailable in one or both snapshots.</p>"),
            Some(changes) if changes.is_empty() => {
                html.push_str("<p>No recorded version changes.</p>")
            }
            Some(changes) => {
                html.push_str("<table><thead><tr><th scope=\"col\">Product</th><th scope=\"col\">Before</th><th scope=\"col\">After</th></tr></thead><tbody>");
                for change in changes {
                    let _ = writeln!(
                        html,
                        "<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td></tr>",
                        escape(&change.label),
                        escape(change.before.as_deref().unwrap_or("Absent")),
                        escape(change.after.as_deref().unwrap_or("Absent"))
                    );
                }
                html.push_str("</tbody></table>");
            }
        }
        html.push_str("</section>");
    }
    html.push_str(PRINT_FOOT);
    html
}

pub fn render_support(session: &Session, snapshots: &[(&str, String)], demo: bool) -> String {
    let mut html = report_start("Support report");
    html.push_str(PRINT_BUTTON);
    html.push_str(
        "</header><section aria-labelledby=\"share\"><h2 id=\"share\">What you’re sharing</h2>",
    );
    if demo {
        html.push_str("<p>Fictional Windows events and hardware.</p>");
    }
    html.push_str("<p>Review this file before sharing. It contains the Windows event types, UTC timestamps, record references and code fields shown below.");
    if !snapshots.is_empty() {
        html.push_str(" You included additional system details.");
    }
    html.push_str("</p><ul><li><a href=\"#support-events\">Redacted event timeline</a></li>");
    for (index, (title, _)) in snapshots.iter().enumerate() {
        let _ = writeln!(
            html,
            "<li><a href=\"#snapshot-{index}\">{}</a></li>",
            escape(title)
        );
    }
    html.push_str("</ul></section><section aria-labelledby=\"support-events\"><h2 id=\"support-events\">Redacted event timeline</h2>");
    write_window(&mut html, session.window);
    write_notices(&mut html, session);
    write_events(&mut html, &session.events, "", false, None, true);
    html.push_str("</section>");
    for (index, (title, content)) in snapshots.iter().enumerate() {
        let _ = writeln!(
            html,
            "<section aria-labelledby=\"snapshot-{index}\"><h2 id=\"snapshot-{index}\">{}</h2><pre tabindex=\"0\">{}</pre></section>",
            escape(title),
            escape(content)
        );
    }
    html.push_str(FOOT);
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incidents::demo;

    #[test]
    fn event_report_has_one_next_step_and_only_curated_help_links() {
        let session = Session {
            events: crate::incidents::DemoCase::get("restart").unwrap().events(),
            window: None,
            notices: vec![],
        };
        let html = render("Windows System log", &session, false);
        assert_eq!(
            html.matches("<strong>What can I do next?</strong>").count(),
            session.events.len()
        );
        for event in &session.events {
            assert!(html.contains(event.kind.next_step()));
            if let Some(article) = event.kind.help() {
                assert!(html.contains(&format!(
                    "href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\"",
                    article.url
                )));
            }
        }
    }

    #[test]
    fn fictional_case_report_links_next_check_to_its_evidence() {
        for case in crate::incidents::DEMO_CASES {
            let session = Session {
                events: case.events(),
                window: None,
                notices: vec![],
            };
            let html = render_demo(&session, case);
            let event = session
                .events
                .iter()
                .find(|event| event.kind == case.evidence)
                .unwrap();
            let anchor = format!("{}-{}", event.kind.channel(), event.record);
            assert!(html.contains(&format!("<h1>{}</h1>", case.title)));
            assert!(html.contains(&format!("href=\"#{anchor}\" data-evidence-link")));
            assert!(html.contains(&format!("<article id=\"{anchor}\"")));
            assert!(html.find(case.next).unwrap() < html.find("<h2 id=\"timeline\">").unwrap());
        }
    }

    #[test]
    fn output_includes_evidence_and_escapes_untrusted_text() {
        let html = render(
            "Demo",
            &Session {
                events: demo(),
                notices: vec!["<script>oops</script>".into()],
                window: None,
            },
            false,
        );
        assert!(html.contains("&lt;script&gt;oops&lt;/script&gt;"));
        assert!(html.contains("System · Microsoft-Windows-Kernel-Power · Event ID 41 · Record 4"));
        assert_eq!(html.matches("shortly after this restart").count(), 1);
        assert!(html.contains("href=\"#System-5\""));
        assert!(html.contains("DRIVER_POWER_STATE_FAILURE"));
        assert!(!html.contains("demo-editor.exe"));
    }

    #[test]
    fn comparison_links_evidence_and_flags_incomplete_logs() {
        let events = demo();
        let baseline = Session {
            events: vec![events[0].clone()],
            notices: vec!["System log unavailable".into()],
            window: None,
        };
        let incident = Session {
            events,
            notices: vec![],
            window: None,
        };
        let html = render_comparison("Demo", &incident, &baseline, true, [None; 2]);
        assert!(html.contains("Treat these counts as partial"));
        assert!(html.contains("href=\"#incident-Application-2\">2</a></td><td>0</td><td>+2</td>"));
        assert!(html.contains("href=\"#baseline-System-1\">1</a></td><td>0</td>"));
        assert!(html.contains("href=\"#incident-System-5\""));
        assert_eq!(html.matches("id=\"incident-System-1\"").count(), 1);
        assert_eq!(html.matches("id=\"baseline-System-1\"").count(), 1);
        assert!(html.contains("demo-editor.exe"));
    }

    #[test]
    fn support_report_includes_only_selected_sections() {
        let session = Session {
            events: demo(),
            notices: vec![],
            window: None,
        };
        let report = render_support(&session, &[("System health", "<local value>".into())], true);
        assert!(report.contains("href=\"#snapshot-0\">System health"));
        assert!(report.contains("&lt;local value&gt;"));
        assert!(!report.contains("Device inventory"));
        assert!(!report.contains("demo-editor.exe"));
    }
    #[test]
    fn history_report_matches_the_grouped_analysis() {
        use crate::crash_groups::{Clues, analyze, demo_events, demo_now};
        for names in [true, false] {
            let session = Session {
                window: None,
                events: demo_events(),
                notices: vec![],
            };
            let analysis = analyze(&session.events, demo_now(), names, Clues::default());
            let html = render_history("Fictional", &session, &analysis, names);
            assert!(html.contains(&format!("data-incidents=\"{}\"", analysis.incidents)));
            for group in &analysis.groups {
                assert!(html.contains(&format!(
                    "id=\"{}\" data-group=\"{}\" data-incidents=\"{}\"",
                    group.key, group.basis, group.incidents
                )));
                for record in &group.records {
                    let anchor = format!("{}-{}", record.channel, record.record);
                    assert!(html.contains(&format!("href=\"#{anchor}\"")));
                    assert!(html.contains(&format!("id=\"{anchor}\"")));
                }
            }
            assert_eq!(html.contains("demo-renderer.dll"), names);
        }
    }
}
