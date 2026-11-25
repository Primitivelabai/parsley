# Parsley usage guide

Parsley is a portable Windows diagnostics app with a Rust core. Rewind shows what Windows recorded around a crash, restart or hardware error. The desktop runs as the current user and collects read-only information. A short-lived administrator collector is available for an on-demand bandwidth capture.

## Try it

Extract the portable Windows x64 package and start `parsley-desktop.exe`. On **Home**, select **Check the last 24 hours** to see recent records, or **Choose a time** to look around a specific moment. Parsley uses Windows WebView2 for its desktop interface. If it cannot start, Parsley opens its classic native interface. Use `parsley-desktop.exe --classic` to open that interface directly.

Under **What happened**, use **Story**, **All events**, **Crashes** and **Compare**. **All events** matches words, providers and record numbers in the window selected in Story, or the latest 24 hours.

Open **How to and help**, then expand **Practice with example data** and choose a case. Read **Next check**, then select **Show the evidence** to open the Windows record behind it. Select **Save report** to keep an HTML copy. Open it from the save confirmation or from **Home** under **Reopen a local report**. Selecting a sidebar section takes you back to the tools for your own PC.

Open **Live → Health** to see a current reading from this PC. Select **Start monitoring** to watch the readings change and see which apps keep using the most memory.

The separate command-line executable has the same report engine. From PowerShell:

```powershell
.\parsley.exe demo demo.html
.\parsley.exe demo-case graphics graphics-driver-reset.html
.\parsley.exe demo-case application crashing-application.html
.\parsley.exe demo-case restart unexplained-restart.html
.\parsley.exe scan last-day.html
.\parsley.exe scan rewind.html --at 2026-11-22T14:14:00+08:00 --minutes 10
.\parsley.exe import .\System.evtx imported.html
.\parsley.exe demo-compare comparison-demo.html
.\parsley.exe compare comparison.html --at 2026-11-22T14:14:00+08:00 --baseline 2026-11-21T14:14:00+08:00
.\parsley.exe history history.html --days 30
.\parsley.exe history history.html --days 90 --health capture.json --versions before.json after.json
.\parsley.exe demo-support support-example.html --include-health --include-inventory
.\parsley.exe demo-compare comparison.json
.\parsley.exe health health.html
.\parsley.exe inventory inventory.html
.\parsley.exe inventory-save before.json
.\parsley.exe inventory-compare before.json after.json versions.html
.\parsley.exe watch
.\parsley.exe watch-health --samples 10
.\parsley.exe watch-health --samples 120 --capture readings.json
.\parsley.exe start-recording recording.jsonl --minutes 15
.\parsley.exe recording-status recording.jsonl
.\parsley.exe stop-recording recording.jsonl
.\parsley.exe connections process-network.html
.\parsley.exe demo-bandwidth bandwidth-demo.html
.\parsley.exe bandwidth bandwidth.html
.\parsley.exe evidence System 1234
```

Open the new HTML report in any modern browser. The demo uses fictional events. Scan uses the current user's existing access to the System and Application logs, with no service or elevation request. Import uses Windows' Event Log API to read an exported log. Each output must be a new file.

Compare uses equal, non-overlapping windows centred on the two timestamps. It shows event counts, their differences and evidence links for both windows. Pass `--from INPUT.evtx` to compare two windows from one exported log.

In **All events**, use **Bookmark** on an event or **Save search** on a typed query. Open these again under **Saved searches and bookmarks**. Parsley stores queries and record references under `%LOCALAPPDATA%\Parsley`. Bookmark lookup checks the original timestamp.

In **Live → Health**, use **Save these readings** to save up to 3,600 recent samples. In **Compare**, attach one capture for both windows or attach a separate baseline capture. From the CLI, use `--health incident.json` and optionally `--baseline-health baseline.json`. The report shows sample coverage and unavailable metrics.

Under **Keep watch while you’re away**, record for 1–60 minutes while you remain signed in, including after the desktop window closes. Check or stop the most recent recording, reopen an earlier one, or browse to the original `.jsonl` file. Attach that file in Compare while recording is active or after it ends. Each complete sample remains available if the recording is interrupted. Windows sign-out and restart end the worker.

When a comparison includes health data, event details show the closest sample within ten seconds of the event. The time offset stays visible alongside the measurements.

Event reports use UTC and include the event type, provider, record number and timestamp. Application and module names are opt-in for HTML reports from real logs. CSV and JSON export selected event metadata. HTML event reports can be printed or saved as PDF in the browser. Raw evidence is viewed separately and can contain private information. Health and inventory snapshots can also expose details about devices or installed software, so review them before sharing.

Live monitoring covers Windows events and CPU, memory, disk, physical-network and GPU-engine activity where Windows exposes those counters. Select a live event to open its raw Windows record. Live health can flag sustained high CPU or memory use. The on-demand health snapshot includes per-process CPU and memory. Inventory includes devices, monitors, batteries, storage reliability, installed desktop software and services. The snapshot collectors use Windows PowerShell and local CIM providers.

In **Live → Network**, use **Who’s my PC talking to?** to sample active TCP connections five times over about eight seconds. Open an app group to see its destinations and how many readings showed each connection. Parsley labels names it could only get from the Windows process list and shows services Windows listed under the same process ID.

Parsley checks recent DNS events if Sysmon is already running and its DNS log is readable. A domain can appear as a possible match when a query from the same process returned the destination IP. Windows’ shared DNS cache can also suggest a domain. Published GitHub and Cloudflare IP ranges identify some network operators. An operator can host many different sites. Purpose labels come from Microsoft’s documented endpoints. Connection readings contain no message contents and can’t establish whether an app was tracking you. Results stay on the screen and are excluded from saved reports.

The support report opens locally for review before sharing. It starts with the redacted event timeline. System health and inventory are separate opt-in sections.

**Save an inventory version snapshot** records Windows, firmware, display driver, storage firmware and installed desktop software versions. **Compare two inventory versions** shows changes between two saved JSON files. Categories Windows could not read are marked unavailable.

**Network connections by process** counts established TCP connections, TCP listeners and UDP endpoints for each process. The report omits addresses and ports. Network byte rates in live health measure the whole physical interface.

**Bandwidth by process (15 seconds)** asks Windows for administrator permission to start a temporary event trace. Run it while the network activity is happening. It ranks observed TCP and UDP traffic by bytes received and sent for each process ID, including local loopback. When Parsley verifies a process across the capture, it includes its executable name. The trace stops on completion and the HTML report is saved locally. The report contains no addresses, ports or packet contents. **Fictional bandwidth capture** opens the same report format with sample data and requires no elevation.

## Save or share an incident case

An incident case saves a time window, selected Windows event metadata and bookmarks as a small JSON file. Use the same file to regenerate an HTML report offline. The case and report omit original logs, computer and account names, paths, raw XML, stack traces and memory.

```powershell
.\parsley.exe case-demo example-case.json
.\parsley.exe case-report example-case.json example-case.html
.\parsley.exe case-bookmark example-case.json bookmarked-case.json 2:Application:2
```

To collect from this PC, use `case-create` with an incident time and the number of minutes on each side. For exported logs, repeat `--evtx` up to eight times. The report numbers sources in the order you supply them. Use a source number, channel and record number from the report to bookmark an event.

```powershell
.\parsley.exe case-create incident.json --at 2026-11-22T14:14:00+08:00 --minutes 15 --evtx system.evtx --evtx application.evtx
.\parsley.exe case-report incident.json incident.html
```

For a blue screen, add `--debugger-report analysis.txt --debugger-at 2026-11-22T14:14:00+08:00` to `case-create`. Use UTF-8 or UTF-16LE text from WinDbg `!analyze -v` containing `BUGCHECK_CODE`, and enter the crash time with its time zone. Add `--include-module` if you want the debugger’s module name saved in the case and HTML. Windows logs its stop record after the restart, so a matching stop code logged from three minutes before to 15 minutes after the crash gives a candidate link. Confirm that both sources came from the same PC. If several records match, the debugger report stays unlinked.

## Build

```powershell
cargo test --locked
cargo build --locked --release --bins
```

For a portable Windows x64 ZIP with the matching committed source, demos and dependency notices, run `node tools/package-windows.mjs OUTPUT_DIR WORK_DIR` on Windows with the Rust MSVC target installed. Pass `--offline` when using the local Cargo cache.

Geist and Geist Mono use the [SIL Open Font License](../ui/OFL-LICENSE.txt).
