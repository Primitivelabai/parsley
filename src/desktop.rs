use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::saved::{Bookmark, Saved, Store};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{COLOR_WINDOW, DEFAULT_GUI_FONT, GetStockObject};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::w;

const MODE: u16 = 101;
const CHOOSE_INPUT: u16 = 102;
const CHOOSE_OUTPUT: u16 = 103;
const RUN: u16 = 104;
const OPEN: u16 = 105;
const LIVE_ROWS: u16 = 106;
const SAVE_SEARCH: u16 = 107;
const SAVE_BOOKMARK: u16 = 108;
const ATTACH_CAPTURE: u16 = 109;
const ATTACH_BASELINE_CAPTURE: u16 = 110;
const REFRESH: usize = 1;
const BC_GET_CHECK: u32 = 0x00F0;
const BC_SET_CHECK: u32 = 0x00F1;
const CHECKED: WPARAM = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Scan,
    Rewind,
    Compare,
    Import,
    Health,
    Inventory,
    Demo,
    DemoCompare,
    DemoHealth,
    DemoInventory,
    LiveTimeline,
    History,
    DemoHistory,
    LiveHealth,
    Evidence,
    Support,
    DemoSupport,
    Bookmarks,
    Searches,
    Connections,
    DemoConnections,
    Bandwidth,
    DemoBandwidth,
    RecordHealth,
    ManageRecording,
    SaveVersions,
    CompareVersions,
    DemoVersions,
}

impl Mode {
    const CHOICES: [(Self, &'static str); 28] = [
        (Self::Scan, "Scan the last 24 hours"),
        (Self::Rewind, "Rewind to a time"),
        (Self::Compare, "Compare two sessions"),
        (Self::Import, "Import an .evtx log"),
        (Self::Health, "System health snapshot"),
        (Self::Inventory, "Device and software inventory"),
        (Self::Demo, "Fictional incident"),
        (Self::DemoCompare, "Fictional session comparison"),
        (Self::DemoHealth, "Fictional system health"),
        (Self::DemoInventory, "Fictional device inventory"),
        (Self::LiveTimeline, "Live timeline"),
        (Self::History, "Crash history over 30 days"),
        (Self::DemoHistory, "Fictional crash history"),
        (Self::LiveHealth, "Live CPU, memory, disk and network"),
        (Self::Evidence, "Inspect a Windows record"),
        (Self::Support, "Create a support report"),
        (Self::DemoSupport, "Fictional support report"),
        (Self::Bookmarks, "Bookmarked Windows records"),
        (Self::Searches, "Saved timeline searches"),
        (Self::Connections, "Network connections by process"),
        (Self::DemoConnections, "Fictional network connections"),
        (Self::Bandwidth, "Bandwidth by process (15 seconds)"),
        (Self::DemoBandwidth, "Fictional bandwidth capture"),
        (Self::RecordHealth, "Record system health in the background"),
        (
            Self::ManageRecording,
            "Check or stop a background recording",
        ),
        (Self::SaveVersions, "Save an inventory version snapshot"),
        (Self::CompareVersions, "Compare two inventory versions"),
        (Self::DemoVersions, "Fictional inventory version comparison"),
    ];

    fn uses_input(self) -> bool {
        matches!(
            self,
            Self::Import
                | Self::Compare
                | Self::Evidence
                | Self::ManageRecording
                | Self::CompareVersions
        )
    }

    fn uses_time(self) -> bool {
        matches!(self, Self::Rewind | Self::Compare)
    }

    fn uses_names(self) -> bool {
        matches!(
            self,
            Self::Scan | Self::Rewind | Self::Compare | Self::Import | Self::History
        )
    }

    fn is_live(self) -> bool {
        matches!(self, Self::LiveTimeline | Self::LiveHealth)
    }

    fn has_output(self) -> bool {
        !self.is_live()
            && !self.is_saved()
            && !matches!(self, Self::Evidence | Self::ManageRecording)
    }

    fn is_support(self) -> bool {
        matches!(self, Self::Support | Self::DemoSupport)
    }

    fn is_saved(self) -> bool {
        matches!(self, Self::Bookmarks | Self::Searches)
    }

    fn index(self) -> usize {
        Self::CHOICES
            .iter()
            .position(|(mode, _)| *mode == self)
            .expect("each mode has a choice")
    }

    fn html_only(self) -> bool {
        self.is_support()
            || matches!(
                self,
                Self::Health
                    | Self::Inventory
                    | Self::DemoHealth
                    | Self::DemoInventory
                    | Self::Connections
                    | Self::DemoConnections
                    | Self::Bandwidth
                    | Self::DemoBandwidth
                    | Self::CompareVersions
                    | Self::DemoVersions
            )
    }

    fn command(self) -> &'static str {
        match self {
            Self::Scan | Self::Rewind => "scan",
            Self::Compare => "compare",
            Self::Import => "import",
            Self::Health => "health",
            Self::Inventory => "inventory",
            Self::Demo => "demo",
            Self::DemoCompare => "demo-compare",
            Self::DemoHealth => "demo-health",
            Self::DemoInventory => "demo-inventory",
            Self::LiveTimeline => "watch",
            Self::History => "history",
            Self::DemoHistory => "demo-history",
            Self::LiveHealth => "watch-health",
            Self::Evidence => "evidence",
            Self::Support => "support",
            Self::DemoSupport => "demo-support",
            Self::Bookmarks | Self::Searches => unreachable!("saved modes use the desktop store"),
            Self::Connections => "connections",
            Self::DemoConnections => "demo-connections",
            Self::Bandwidth => "bandwidth",
            Self::DemoBandwidth => "demo-bandwidth",
            Self::RecordHealth => "start-recording",
            Self::ManageRecording => "recording-status",
            Self::SaveVersions => "inventory-save",
            Self::CompareVersions => "inventory-compare",
            Self::DemoVersions => "demo-inventory-compare",
        }
    }
}

struct Controls {
    mode: HWND,
    source: HWND,
    source_label: HWND,
    recording_file: HWND,
    before_versions: HWND,
    input_button: HWND,
    at: HWND,
    at_label: HWND,
    baseline: HWND,
    baseline_label: HWND,
    help: HWND,
    capture_label: HWND,
    capture_file: HWND,
    after_versions: HWND,
    capture_picker: HWND,
    baseline_capture_label: HWND,
    baseline_capture: HWND,
    baseline_capture_picker: HWND,
    minutes: HWND,
    minutes_label: HWND,
    duration: HWND,
    include_names: HWND,
    include_inventory: HWND,
    output: HWND,
    output_label: HWND,
    output_button: HWND,
    run: HWND,
    open: HWND,
    status: HWND,
    live_list: HWND,
    live_info: HWND,
    save_search: HWND,
    save_bookmark: HWND,
}

struct State {
    controls: Controls,
    pending: Option<Receiver<JobResult>>,
    completed: Option<PathBuf>,
    live: Option<Receiver<Result<crate::live::Batch, String>>>,
    live_lines: VecDeque<crate::live::Line>,
    live_rows: Vec<crate::live::Line>,
    store: Result<Store, String>,
    saved: Saved,
    health_samples: VecDeque<crate::capture::Sample>,
    capture_status: Option<String>,
}

type JobResult = (Option<PathBuf>, Result<String, String>);

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub fn main() -> std::process::ExitCode {
    if let Some(exit) = crate::network::maybe_worker() {
        return exit;
    }
    if let Some(exit) = crate::background::maybe_worker() {
        return exit;
    }
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    let recording_smoke = command
        .as_ref()
        .is_some_and(|arg| arg == "--smoke-recording");
    let smoke = recording_smoke || command.is_some_and(|arg| arg == "--smoke");
    let smoke_output = smoke.then(|| args.next().map(PathBuf::from)).flatten();
    // Win32 window handles and messages stay on this thread.
    match unsafe { run(smoke, smoke_output, recording_smoke) } {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if smoke {
                eprintln!("Parsley desktop smoke: {error}");
            } else {
                let error = wide(error.as_ref());
                unsafe {
                    MessageBoxW(
                        null_mut(),
                        error.as_ptr(),
                        w!("Parsley"),
                        MB_OK | MB_ICONERROR,
                    )
                };
            }
            std::process::ExitCode::FAILURE
        }
    }
}

pub(crate) fn smoke(output: PathBuf) -> Result<(), String> {
    // The caller invokes the native smoke from the desktop UI thread.
    unsafe { run(true, Some(output), false) }
}

unsafe fn run(
    smoke: bool,
    smoke_output: Option<PathBuf>,
    recording_smoke: bool,
) -> Result<(), String> {
    unsafe {
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as usize as _,
            lpszClassName: w!("ParsleyDesktop"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err("Could not register the Parsley window.".into());
        }
        let window = CreateWindowExW(
            0,
            class.lpszClassName,
            w!("Parsley"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            760,
            650,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );
        if window.is_null() {
            return Err("Could not open the Parsley window.".into());
        }
        let controls = Controls::new(window)?;
        controls.update_mode();
        STATE.with(|state| {
            let store = if smoke {
                smoke_output
                    .as_ref()
                    .map(|path| Store::new(path.with_extension("saved.json")))
                    .ok_or_else(|| "No isolated desktop smoke store was supplied.".into())
            } else {
                Store::for_user()
            };
            *state.borrow_mut() = Some(State {
                controls,
                pending: None,
                completed: None,
                live: None,
                live_lines: VecDeque::new(),
                live_rows: Vec::new(),
                store,
                saved: Saved::default(),
                health_samples: VecDeque::new(),
                capture_status: None,
            });
        });
        if smoke {
            let result = if recording_smoke {
                smoke_background(smoke_output.ok_or("Choose a file for the recorder check.")?)
            } else {
                smoke_controls(smoke_output)
            };
            DestroyWindow(window);
            return result;
        }
        if SetTimer(window, REFRESH, 120, None) == 0 {
            DestroyWindow(window);
            return Err("Could not initialise the Parsley refresh timer.".into());
        }
        ShowWindow(window, SW_SHOW);
        let mut message = MSG::default();
        loop {
            let status = GetMessageW(&mut message, null_mut(), 0, 0);
            if status == 0 {
                return Ok(());
            }
            if status < 0 {
                return Err("Windows stopped the Parsley message loop.".into());
            }
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

fn smoke_background(output: PathBuf) -> Result<(), String> {
    STATE.with(|cell| -> Result<(), String> {
        let mut state = cell.borrow_mut();
        let state = state.as_mut().ok_or("Missing desktop state.")?;
        unsafe {
            SendMessageW(
                state.controls.mode,
                CB_SETCURSEL,
                Mode::RecordHealth.index(),
                0,
            )
        };
        state.change_mode();
        let path = wide(output.as_os_str());
        unsafe { SetWindowTextW(state.controls.output, path.as_ptr()) };
        text(state.controls.duration, "1");
        state.start();
        Ok(())
    })?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(7);
    while std::time::Instant::now() < deadline {
        let result = STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            let state = state
                .as_mut()
                .expect("desktop state exists through the recording check");
            state.poll();
            (
                state.pending.is_none(),
                state.controls.mode() == Mode::ManageRecording,
                read(state.controls.recording_file),
                state
                    .store
                    .as_ref()
                    .ok()
                    .and_then(|store| store.read().ok())
                    .and_then(|saved| saved.last_recording),
            )
        });
        if result.0 {
            let expected = std::path::absolute(&output).map_err(|error| error.to_string())?;
            if result.1
                && result.2 == expected.as_os_str()
                && result.3.as_deref() == expected.to_str()
            {
                return Ok(());
            }
            return Err("The desktop did not open the recording controls.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err("The desktop recorder did not start.".into())
}

unsafe extern "system" fn window_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam & 0xffff) as u16;
            let notification = ((wparam >> 16) & 0xffff) as u16;
            // File pickers use a nested message loop, so release STATE before invoking one.
            let save_health = id == SAVE_SEARCH
                && STATE.with(|state| {
                    state
                        .try_borrow()
                        .ok()
                        .and_then(|state| {
                            state
                                .as_ref()
                                .map(|state| state.controls.mode() == Mode::LiveHealth)
                        })
                        .unwrap_or(false)
                });
            if notification == 0
                && (matches!(
                    id,
                    CHOOSE_INPUT | CHOOSE_OUTPUT | ATTACH_CAPTURE | ATTACH_BASELINE_CAPTURE
                ) || save_health)
            {
                choose_path(
                    window,
                    id == CHOOSE_OUTPUT || save_health,
                    matches!(id, ATTACH_CAPTURE | ATTACH_BASELINE_CAPTURE) || save_health,
                    id == ATTACH_BASELINE_CAPTURE,
                );
                return 0;
            }
            handle_command(window, id, notification);
            0
        }
        WM_TIMER if wparam == REFRESH => {
            STATE.with(|state| {
                if let Ok(mut slot) = state.try_borrow_mut()
                    && let Some(state) = slot.as_mut()
                {
                    state.poll();
                }
            });
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(window, msg, wparam, lparam) },
    }
}

fn handle_command(window: HWND, id: u16, notification: u16) {
    if (id, notification) == (OPEN, 0) || (id, notification) == (LIVE_ROWS, LBN_DBLCLK as u16) {
        let path = STATE.with(|state| {
            let mut slot = state.try_borrow_mut().ok()?;
            let state = slot.as_mut()?;
            if state.live.is_some() && state.controls.mode() == Mode::LiveTimeline {
                state.open_live_record();
                return None;
            }
            if state.controls.mode() == Mode::ManageRecording && id == OPEN {
                state.stop_recording();
                return None;
            }
            if state.controls.mode().is_saved() {
                if id == OPEN {
                    state.remove_saved();
                } else {
                    state.start();
                }
                return None;
            }
            state.completed.clone()
        });
        if let Some(path) = path {
            let path = wide(path.as_os_str());
            let handle = unsafe {
                ShellExecuteW(window, w!("open"), path.as_ptr(), null(), null(), SW_SHOW)
            };
            if handle as usize <= 32 {
                STATE.with(|state| {
                    if let Some(state) = state.borrow().as_ref() {
                        text(state.controls.status, "Windows could not open the report.");
                    }
                });
            }
        }
        return;
    }
    STATE.with(|state| {
        if let Ok(mut slot) = state.try_borrow_mut()
            && let Some(state) = slot.as_mut()
        {
            match (id, notification) {
                (MODE, 1) => state.change_mode(),
                (RUN, 0) => state.start(),
                (SAVE_SEARCH, 0) => state.save_search(),
                (SAVE_BOOKMARK, 0) => state.save_bookmark(),
                _ => {}
            }
        }
    });
}

impl Controls {
    fn new(parent: HWND) -> Result<Self, String> {
        let label = |name, y| control(parent, w!("STATIC"), name, 0, 0, (16, y, 190, 22));
        let edit = |name, y| {
            control(
                parent,
                w!("EDIT"),
                name,
                WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
                0,
                (205, y, 390, 25),
            )
        };
        let button = |name, id, x, y, width| {
            control(
                parent,
                w!("BUTTON"),
                name,
                WS_TABSTOP,
                id,
                (x, y, width, 27),
            )
        };
        label("Action", 20)?;
        let mode = control(
            parent,
            w!("COMBOBOX"),
            "",
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            MODE,
            (205, 16, 390, 300),
        )?;
        for (_, title) in Mode::CHOICES {
            let title = wide(title.as_ref());
            if unsafe { SendMessageW(mode, CB_ADDSTRING, 0, title.as_ptr() as isize) } < 0 {
                return Err("Could not load the list of Parsley actions.".into());
            }
        }
        unsafe { SendMessageW(mode, CB_SETCURSEL, 0, 0) };
        let source_label = label("Event log file", 58)?;
        let source = edit("", 54)?;
        let recording_file = edit("", 54)?;
        let before_versions = edit("", 54)?;
        let input_button = button("Browse…", CHOOSE_INPUT, 610, 53, 110)?;
        let at_label = label("Incident time", 94)?;
        let at = edit("", 90)?;
        let baseline_label = label("Baseline time", 130)?;
        let baseline = edit("", 126)?;
        let minutes_label = label("Minutes each side", 166)?;
        let minutes = edit("10", 162)?;
        unsafe {
            SetWindowPos(
                minutes,
                null_mut(),
                205,
                162,
                55,
                25,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        let duration = control(
            parent,
            w!("EDIT"),
            "15",
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
            0,
            (205, 162, 55, 25),
        )?;
        let baseline_capture_label = control(
            parent,
            w!("STATIC"),
            "Baseline capture",
            0,
            0,
            (275, 165, 125, 25),
        )?;
        let baseline_capture = control(
            parent,
            w!("EDIT"),
            "",
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
            0,
            (400, 162, 195, 25),
        )?;
        let baseline_capture_picker = button("Browse…", ATTACH_BASELINE_CAPTURE, 610, 161, 110)?;
        let help = control(
            parent,
            w!("STATIC"),
            "Enter a timestamp with a time zone, e.g. 2026-11-22T14:14:00+08:00.",
            0,
            0,
            (205, 193, 515, 21),
        )?;
        let capture_label = label("Incident health capture", 194)?;
        let capture_file = edit("", 190)?;
        let after_versions = edit("", 190)?;
        let capture_picker = button("Browse…", ATTACH_CAPTURE, 610, 189, 110)?;
        let include_names = control(
            parent,
            w!("BUTTON"),
            "Include application and module names in this report",
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            0,
            (205, 220, 510, 27),
        )?;
        let include_inventory = control(
            parent,
            w!("BUTTON"),
            "Include device inventory",
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            0,
            (455, 220, 265, 27),
        )?;
        show(include_inventory, false);
        let output_label = label("Save report to", 265)?;
        let output = edit("", 261)?;
        let output_button = button("Choose…", CHOOSE_OUTPUT, 610, 260, 110)?;
        let run = button("Create report", RUN, 205, 300, 136)?;
        let open = button("Open report", OPEN, 350, 300, 172)?;
        let save_bookmark = button("Bookmark", SAVE_BOOKMARK, 530, 300, 89)?;
        let save_search = button("Save search", SAVE_SEARCH, 627, 300, 93)?;
        show(save_bookmark, false);
        show(save_search, false);
        let status = control(
            parent,
            w!("EDIT"),
            "Choose an action. Reports are saved on this PC.",
            WS_TABSTOP
                | WS_BORDER
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            0,
            (16, 350, 704, 118),
        )?;
        let live_info = control(parent, w!("STATIC"), "", 0, 0, (16, 333, 704, 22))?;
        let live_list = control(
            parent,
            w!("LISTBOX"),
            "",
            WS_TABSTOP
                | WS_BORDER
                | WS_VSCROLL
                | WS_HSCROLL
                | LBS_NOTIFY as u32
                | LBS_NOINTEGRALHEIGHT as u32,
            LIVE_ROWS,
            (16, 357, 704, 128),
        )?;
        unsafe { SendMessageW(live_list, LB_SETHORIZONTALEXTENT, 1200, 0) };
        show(live_list, false);
        show(live_info, false);
        enable(open, false);
        Ok(Self {
            mode,
            source,
            source_label,
            recording_file,
            before_versions,
            input_button,
            at,
            at_label,
            baseline,
            baseline_label,
            help,
            capture_label,
            capture_file,
            after_versions,
            capture_picker,
            baseline_capture_label,
            baseline_capture,
            baseline_capture_picker,
            minutes,
            minutes_label,
            duration,
            include_names,
            include_inventory,
            output,
            output_label,
            output_button,
            run,
            open,
            status,
            live_list,
            live_info,
            save_search,
            save_bookmark,
        })
    }

    fn mode(&self) -> Mode {
        let index = unsafe { SendMessageW(self.mode, CB_GETCURSEL, 0, 0) };
        Mode::CHOICES
            .get(index as usize)
            .map_or(Mode::Scan, |(mode, _)| *mode)
    }

    fn update_mode(&self) {
        let mode = self.mode();
        show(
            self.source,
            !matches!(mode, Mode::ManageRecording | Mode::CompareVersions),
        );
        show(self.recording_file, mode == Mode::ManageRecording);
        show(self.before_versions, mode == Mode::CompareVersions);
        text(
            self.source_label,
            if mode == Mode::ManageRecording {
                "Recording file"
            } else if mode == Mode::CompareVersions {
                "Before snapshot"
            } else {
                "Event log file"
            },
        );
        text(
            self.output_label,
            if mode == Mode::RecordHealth {
                "Save recording to"
            } else if mode == Mode::SaveVersions {
                "Save version snapshot to"
            } else {
                "Save report to"
            },
        );
        text(
            self.minutes_label,
            if mode == Mode::RecordHealth {
                "Recording minutes"
            } else {
                "Minutes each side"
            },
        );
        show(self.minutes, mode != Mode::RecordHealth);
        show(self.duration, mode == Mode::RecordHealth);
        show(
            self.help,
            !matches!(mode, Mode::Compare | Mode::CompareVersions),
        );
        show(
            self.capture_label,
            matches!(mode, Mode::Compare | Mode::CompareVersions),
        );
        text(
            self.capture_label,
            if mode == Mode::CompareVersions {
                "After snapshot"
            } else {
                "Incident health capture"
            },
        );
        show(self.capture_file, mode == Mode::Compare);
        show(self.after_versions, mode == Mode::CompareVersions);
        show(
            self.capture_picker,
            matches!(mode, Mode::Compare | Mode::CompareVersions),
        );
        show(self.baseline_capture_label, mode == Mode::Compare);
        show(self.baseline_capture, mode == Mode::Compare);
        show(self.baseline_capture_picker, mode == Mode::Compare);
        enable(self.source, mode.uses_input());
        enable(self.input_button, mode.uses_input());
        enable(
            self.at,
            mode.uses_time() || matches!(mode, Mode::Evidence | Mode::LiveTimeline),
        );
        enable(
            self.baseline,
            matches!(mode, Mode::Compare | Mode::Evidence),
        );
        enable(self.minutes, mode.uses_time());
        enable(
            self.include_names,
            mode.uses_names() || mode.is_support() || mode == Mode::LiveHealth,
        );
        show(self.include_inventory, mode.is_support());
        text(
            self.include_names,
            if mode.is_support() {
                "Include system health snapshot"
            } else if mode == Mode::LiveHealth {
                "Flag sustained high CPU or physical memory"
            } else {
                "Include application and module names in this report"
            },
        );
        unsafe {
            SendMessageW(self.include_names, BC_SET_CHECK, 0, 0);
            SendMessageW(self.include_inventory, BC_SET_CHECK, 0, 0);
            SetWindowPos(
                self.include_names,
                null_mut(),
                205,
                220,
                if mode.is_support() { 248 } else { 510 },
                27,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        enable(self.output, mode.has_output());
        enable(self.output_button, mode.has_output());
        text(
            self.at_label,
            if mode == Mode::Evidence {
                "Channel"
            } else if mode == Mode::LiveTimeline {
                "Search live events"
            } else {
                "Incident time"
            },
        );
        text(
            self.baseline_label,
            if mode == Mode::Evidence {
                "Windows record number"
            } else {
                "Baseline time"
            },
        );
        text(
            self.help,
            if mode == Mode::Evidence {
                "Enter System or Application. Raw records may contain private information."
            } else if mode == Mode::LiveTimeline {
                "Search names, explanations or Windows record numbers while monitoring."
            } else if mode.is_support() {
                "Includes selected event metadata. Additional snapshots are optional."
            } else if mode == Mode::RecordHealth {
                "Select a new file and 1–60 minutes. The window can close while recording."
            } else if mode == Mode::ManageRecording {
                "Choose the original .jsonl file to check or stop its recorder."
            } else if mode == Mode::SaveVersions {
                "Save Windows, firmware, driver and software versions as a local JSON snapshot."
            } else if mode == Mode::Bandwidth {
                "Windows asks for admin permission. The report saves process names and byte totals locally."
            } else if mode.uses_time() {
                "Enter a timestamp with a time zone, e.g. 2026-11-22T14:14:00+08:00."
            } else {
                ""
            },
        );
        text(
            self.run,
            if mode.is_live() {
                "Start monitoring"
            } else if mode == Mode::Evidence {
                "Read record"
            } else if mode == Mode::Bookmarks {
                "Read bookmark"
            } else if mode == Mode::Searches {
                "Use saved search"
            } else if mode == Mode::RecordHealth {
                "Start recording"
            } else if mode == Mode::ManageRecording {
                "Check recording"
            } else if mode == Mode::SaveVersions {
                "Save snapshot"
            } else if mode == Mode::CompareVersions {
                "Compare snapshots"
            } else if mode == Mode::Bandwidth {
                "Capture bandwidth"
            } else {
                "Create report"
            },
        );
    }

    fn request(&self) -> Result<(Option<PathBuf>, Vec<OsString>), String> {
        let mode = self.mode();
        if mode == Mode::ManageRecording {
            let path = read(self.recording_file);
            if path.is_empty() {
                return Err("Choose the original .jsonl recording file.".into());
            }
            return Ok((None, vec![mode.command().into(), path]));
        }
        if mode == Mode::RecordHealth {
            let path = read(self.output);
            if path.is_empty() {
                return Err("Choose a new .jsonl recording file.".into());
            }
            let path =
                std::path::absolute(PathBuf::from(path)).map_err(|error| error.to_string())?;
            let args = vec![
                mode.command().into(),
                path.as_os_str().to_owned(),
                "--minutes".into(),
                read(self.duration),
            ];
            return Ok((Some(path), args));
        }
        if mode == Mode::Evidence {
            let mut args = vec![
                OsString::from("evidence"),
                read(self.at),
                read(self.baseline),
            ];
            let source = read(self.source);
            if !source.is_empty() {
                args.extend([OsString::from("--from"), source]);
            }
            return Ok((None, args));
        }
        let path = PathBuf::from(read(self.output));
        if path.as_os_str().is_empty() {
            return Err("Choose a path for a new report.".into());
        }
        let path = std::path::absolute(path).map_err(|error| error.to_string())?;
        let mut args = vec![OsString::from(mode.command())];
        if mode == Mode::CompareVersions {
            let before = read(self.before_versions);
            let after = read(self.after_versions);
            if before.is_empty() || after.is_empty() {
                return Err("Choose the before and after version snapshots.".into());
            }
            args.extend([before, after]);
        }
        let source = read(self.source);
        if mode == Mode::Import {
            if source.is_empty() {
                return Err("Choose an exported .evtx file to import.".into());
            }
            args.push(source.clone());
        }
        args.push(path.as_os_str().to_owned());
        if mode.is_support() {
            if checked(self.include_names) {
                args.push("--include-health".into());
            }
            if checked(self.include_inventory) {
                args.push("--include-inventory".into());
            }
        }
        if mode.uses_time() {
            let at = read(self.at);
            if at.is_empty() {
                return Err("Enter the incident time with its time zone.".into());
            }
            args.extend([OsString::from("--at"), at]);
            let minutes = read(self.minutes);
            if !minutes.is_empty() {
                args.extend([OsString::from("--minutes"), minutes]);
            }
        }
        if mode == Mode::Compare {
            let baseline = read(self.baseline);
            if baseline.is_empty() {
                return Err("Enter the baseline time with its time zone.".into());
            }
            args.extend([OsString::from("--baseline"), baseline]);
            if !source.is_empty() {
                args.extend([OsString::from("--from"), source]);
            }
            let path = read(self.capture_file);
            if !path.is_empty() {
                args.extend([OsString::from("--health"), path]);
            }
            let baseline = read(self.baseline_capture);
            if !baseline.is_empty() {
                args.extend([OsString::from("--baseline-health"), baseline]);
            }
        }
        if mode.uses_names() && checked(self.include_names) {
            args.push("--include-app-names".into());
        }
        Ok((Some(path), args))
    }
}

impl State {
    fn change_mode(&mut self) {
        self.controls.update_mode();
        let mode = self.controls.mode();
        if mode == Mode::ManageRecording
            && read(self.controls.recording_file).is_empty()
            && let Ok(saved) = self
                .store
                .as_ref()
                .map_err(Clone::clone)
                .and_then(Store::read)
            && let Some(last) = saved.last_recording
        {
            text(self.controls.recording_file, &last);
        }
        self.live_layout(mode.is_saved());
        text(
            self.controls.open,
            if mode.is_saved() {
                "Remove selected"
            } else if mode == Mode::ManageRecording {
                "Stop recording"
            } else {
                "Open report"
            },
        );
        enable(self.controls.run, self.pending.is_none());
        enable(
            self.controls.open,
            self.pending.is_none()
                && (mode == Mode::ManageRecording
                    || (!mode.is_saved() && self.completed.is_some())),
        );
        if mode.is_saved() {
            match self
                .store
                .as_ref()
                .map_err(Clone::clone)
                .and_then(Store::read)
            {
                Ok(saved) => {
                    self.saved = saved;
                    self.render_saved();
                    text(self.controls.status, "");
                }
                Err(error) => {
                    self.saved = Saved::default();
                    self.render_saved();
                    text(self.controls.status, &error);
                }
            }
        }
        self.offer_health_capture();
    }

    fn render_saved(&self) {
        let mode = self.controls.mode();
        let captions: Vec<_> = if mode == Mode::Bookmarks {
            self.saved.bookmarks.iter().map(Bookmark::caption).collect()
        } else {
            self.saved.searches.clone()
        };
        let list = self.controls.live_list;
        unsafe { SendMessageW(list, LB_RESETCONTENT, 0, 0) };
        for caption in &captions {
            let caption = wide(caption.as_ref());
            if unsafe { SendMessageW(list, LB_ADDSTRING, 0, caption.as_ptr() as isize) } < 0 {
                unsafe { SendMessageW(list, LB_RESETCONTENT, 0, 0) };
                text(
                    self.controls.live_info,
                    "Windows could not display the saved items.",
                );
                enable(self.controls.run, false);
                enable(self.controls.open, false);
                return;
            }
        }
        if !captions.is_empty() {
            unsafe { SendMessageW(list, LB_SETCURSEL, 0, 0) };
        }
        let info = match (mode, captions.is_empty()) {
            (Mode::Bookmarks, true) => "Bookmark a Windows record from the live timeline.",
            (Mode::Bookmarks, false) => "Bookmarks saved on this PC",
            (_, true) => "Save a search from the live timeline.",
            (_, false) => "Searches saved on this PC",
        };
        text(self.controls.live_info, info);
        enable(
            self.controls.run,
            self.pending.is_none() && !captions.is_empty(),
        );
        enable(
            self.controls.open,
            self.pending.is_none() && !captions.is_empty(),
        );
    }

    fn list_selection(&self) -> usize {
        unsafe { SendMessageW(self.controls.live_list, LB_GETCURSEL, 0, 0) as usize }
    }

    fn change_saved(
        &mut self,
        update: impl FnOnce(&mut Saved) -> Result<(), String>,
    ) -> Result<(), String> {
        let store = self.store.as_ref().map_err(Clone::clone)?;
        self.saved = store.change(update)?;
        Ok(())
    }

    fn save_search(&mut self) {
        let value = read(self.controls.at).to_string_lossy().into_owned();
        match self.change_saved(|saved| saved.add_search(&value)) {
            Ok(()) => text(self.controls.status, "Search saved on this PC."),
            Err(error) => text(self.controls.status, &error),
        }
    }

    fn offer_health_capture(&self) {
        if self.controls.mode() == Mode::LiveHealth {
            text(self.controls.save_search, "Save data…");
            show(self.controls.save_search, !self.health_samples.is_empty());
        } else if self.live.is_none() {
            show(self.controls.save_search, false);
        }
    }

    fn save_health(&mut self, path: &std::path::Path) {
        if self.health_samples.is_empty() {
            return;
        }
        let result = (|| {
            if crate::export::Format::from_path(path)? != crate::export::Format::Json {
                return Err("Health captures use .json output.".into());
            }
            let capture = crate::capture::Capture::from_samples(
                self.health_samples.iter().cloned().collect(),
            );
            crate::write_report(path, &capture.json()?)?;
            Ok(format!(
                "Saved {} health samples to {}",
                capture.sample_count(),
                path.display()
            ))
        })();
        let message = result.unwrap_or_else(|error: String| error);
        text(self.controls.status, &message);
        self.capture_status = Some(message);
    }

    fn save_bookmark(&mut self) {
        let Some(marker) = self.live_record_selected().cloned() else {
            return;
        };
        match self.change_saved(move |saved| {
            saved.bookmark(marker);
            Ok(())
        }) {
            Ok(()) => text(
                self.controls.status,
                "Windows record bookmarked on this PC.",
            ),
            Err(error) => text(self.controls.status, &error),
        }
    }

    fn remove_saved(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let index = self.list_selection();
        let result = if self.controls.mode() == Mode::Bookmarks {
            let Some(marker) = self.saved.bookmarks.get(index).cloned() else {
                return;
            };
            self.change_saved(move |saved| {
                saved.remove_bookmark(&marker);
                Ok(())
            })
        } else {
            let Some(value) = self.saved.searches.get(index).cloned() else {
                return;
            };
            self.change_saved(move |saved| {
                saved.searches.retain(|search| search != &value);
                Ok(())
            })
        };
        match result {
            Ok(()) => {
                self.render_saved();
                text(self.controls.status, "Removed from saved items.");
            }
            Err(error) => text(self.controls.status, &error),
        }
    }

    fn start(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let mode = self.controls.mode();
        if mode == Mode::Bookmarks {
            if let Some(marker) = self.saved.bookmarks.get(self.list_selection()).cloned() {
                self.read_marker(marker);
            }
            return;
        }
        if mode == Mode::Searches {
            if let Some(value) = self.saved.searches.get(self.list_selection()).cloned() {
                unsafe {
                    SendMessageW(
                        self.controls.mode,
                        CB_SETCURSEL,
                        Mode::LiveTimeline.index(),
                        0,
                    )
                };
                self.change_mode();
                text(self.controls.at, &value);
                text(
                    self.controls.status,
                    "Saved search loaded. Start monitoring to use it.",
                );
            }
            return;
        }
        if mode.is_live() {
            self.toggle_live(mode);
            return;
        }
        let (path, args) = match self.controls.request() {
            Ok(request) => request,
            Err(error) => {
                text(self.controls.status, &error);
                return;
            }
        };
        let status = if mode == Mode::Evidence {
            "Reading the record…"
        } else if mode == Mode::RecordHealth {
            "Starting the background recorder…"
        } else if mode == Mode::ManageRecording {
            "Checking the recording…"
        } else if mode == Mode::Bandwidth {
            "Approve the Windows prompt to start the 15-second bandwidth capture…"
        } else {
            "Creating the report…"
        };
        self.start_job(path, args, status);
    }

    fn stop_recording(&mut self) {
        if self.pending.is_some() {
            return;
        }
        match self.controls.request() {
            Ok((_, mut args)) => {
                args[0] = "stop-recording".into();
                self.start_job(None, args, "Sending the stop request…");
            }
            Err(error) => text(self.controls.status, &error),
        }
    }

    fn start_job(&mut self, path: Option<PathBuf>, args: Vec<OsString>, status: &str) {
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("parsley-report".into())
            .spawn(move || {
                let result = crate::run(args.into_iter());
                let _ = tx.send((path, result));
            }) {
            Ok(_) => {
                self.pending = Some(rx);
                self.completed = None;
                enable(self.controls.mode, false);
                enable(self.controls.run, false);
                enable(self.controls.open, false);
                text(self.controls.status, status);
            }
            Err(error) => text(
                self.controls.status,
                &format!("Could not start the report. {error}"),
            ),
        }
    }

    fn poll(&mut self) {
        if self.live.is_some() {
            self.poll_live();
        }
        let Some(pending) = &self.pending else { return };
        match pending.try_recv() {
            Ok((path, result)) => {
                self.pending = None;
                enable(self.controls.mode, self.live.is_none());
                enable(self.controls.run, true);
                if self.controls.mode().is_saved() || self.controls.mode() == Mode::ManageRecording
                {
                    enable(self.controls.open, true);
                }
                match result {
                    Ok(message) => {
                        if self.controls.mode() == Mode::RecordHealth {
                            self.completed = None;
                            if let Some(original) = path {
                                if let Some(last) = original.to_str().map(str::to_owned) {
                                    let _ = self.change_saved(move |saved| {
                                        saved.last_recording = Some(last);
                                        Ok(())
                                    });
                                }
                                let original = wide(original.as_os_str());
                                unsafe {
                                    SetWindowTextW(self.controls.recording_file, original.as_ptr());
                                    SendMessageW(
                                        self.controls.mode,
                                        CB_SETCURSEL,
                                        Mode::ManageRecording.index(),
                                        0,
                                    );
                                }
                                self.change_mode();
                            }
                        } else {
                            self.completed = path;
                        }
                        enable(
                            self.controls.open,
                            self.controls.mode().is_saved()
                                || self.controls.mode() == Mode::ManageRecording
                                || self.completed.is_some()
                                || self.live_record_selected().is_some(),
                        );
                        text(self.controls.status, &message);
                    }
                    Err(error) => text(self.controls.status, &error),
                }
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                enable(self.controls.mode, self.live.is_none());
                if self.controls.mode().is_saved() || self.controls.mode() == Mode::ManageRecording
                {
                    enable(self.controls.open, true);
                }
                enable(self.controls.run, true);
                text(
                    self.controls.status,
                    "Report creation stopped unexpectedly.",
                );
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn toggle_live(&mut self, mode: Mode) {
        if self.live.take().is_some() {
            self.finish_live("Monitoring stopped.");
            return;
        }
        let (tx, rx) = mpsc::sync_channel(2);
        let alerts = mode == Mode::LiveHealth && checked(self.controls.include_names);
        match std::thread::Builder::new()
            .name("parsley-live".into())
            .spawn(move || {
                let take = |batch| tx.send(Ok(batch)).is_ok();
                let result = if mode == Mode::LiveHealth {
                    crate::system::watch_health(alerts, take)
                } else {
                    crate::live::watch(take)
                };
                if let Err(error) = result {
                    let _ = tx.send(Err(error));
                }
            }) {
            Ok(_) => {
                self.live = Some(rx);
                self.live_lines.clear();
                self.live_rows.clear();
                unsafe { SendMessageW(self.controls.live_list, LB_RESETCONTENT, 0, 0) };
                enable(self.controls.mode, false);
                enable(self.controls.include_names, false);
                text(self.controls.run, "Stop monitoring");
                if mode == Mode::LiveTimeline {
                    self.live_layout(true);
                    text(self.controls.live_info, "Reading recent Windows events…");
                    text(self.controls.open, "Read selected record");
                    enable(self.controls.open, false);
                    show(self.controls.save_bookmark, true);
                    show(self.controls.save_search, true);
                    text(self.controls.save_search, "Save search");
                    enable(self.controls.save_bookmark, false);
                }
                text(
                    self.controls.status,
                    if mode == Mode::LiveHealth {
                        "Sampling CPU, memory, disk and network…"
                    } else {
                        "Reading the previous ten minutes of supported events…"
                    },
                );
            }
            Err(error) => text(
                self.controls.status,
                &format!("Could not start the timeline. {error}"),
            ),
        }
    }

    fn poll_live(&mut self) {
        let Some(live) = &self.live else { return };
        let mut update = None;
        loop {
            match live.try_recv() {
                Ok(Ok(batch)) => {
                    if let Some(sample) = batch.health {
                        if self.health_samples.len() == crate::capture::MAX_SAMPLES {
                            self.health_samples.pop_front();
                        }
                        self.health_samples.push_back(sample);
                    }
                    self.live_lines.extend(batch.lines);
                    while self.live_lines.len() > 250 {
                        self.live_lines.pop_front();
                    }
                    update = Some(batch.notices);
                }
                Ok(Err(error)) => {
                    self.finish_live(&error);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.finish_live("Monitoring stopped.");
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if let Some(mut notices) = update {
            if self.controls.mode() == Mode::LiveTimeline {
                self.render_live_rows(&notices);
            } else {
                self.offer_health_capture();
                if let Some(status) = &self.capture_status {
                    notices.push(status.clone());
                }
                notices.extend(self.live_lines.iter().rev().map(|line| line.text.clone()));
                text(self.controls.status, &notices.join("\n"));
            }
        }
    }

    fn render_live_rows(&mut self, notices: &[String]) {
        let search = read(self.controls.at)
            .to_string_lossy()
            .trim()
            .to_lowercase();
        let rows: Vec<_> = self
            .live_lines
            .iter()
            .rev()
            .filter(|line| search.is_empty() || line.text.to_lowercase().contains(&search))
            .cloned()
            .collect();
        if rows != self.live_rows {
            let prior = self.live_record_selected().cloned();
            let list = self.controls.live_list;
            unsafe { SendMessageW(list, LB_RESETCONTENT, 0, 0) };
            self.live_rows.clear();
            for row in rows {
                let caption = wide(row.text.as_ref());
                if unsafe { SendMessageW(list, LB_ADDSTRING, 0, caption.as_ptr() as isize) } >= 0 {
                    self.live_rows.push(row);
                }
            }
            if !self.live_rows.is_empty() {
                let selection = self
                    .live_rows
                    .iter()
                    .position(|row| row.reference.as_ref() == prior.as_ref())
                    .unwrap_or_default();
                unsafe { SendMessageW(list, LB_SETCURSEL, selection, 0) };
            }
        }
        let message = if !notices.is_empty() {
            notices.join(" ")
        } else if self.live_rows.is_empty() {
            "Watching. No events match.".into()
        } else {
            format!("Watching. {} events shown.", self.live_rows.len())
        };
        text(self.controls.live_info, &message);
        enable(
            self.controls.open,
            self.pending.is_none() && self.live_record_selected().is_some(),
        );
        enable(
            self.controls.save_bookmark,
            self.live_record_selected().is_some(),
        );
    }

    fn live_record_selected(&self) -> Option<&Bookmark> {
        if self.live.is_none() || self.controls.mode() != Mode::LiveTimeline {
            return None;
        }
        self.live_rows
            .get(self.list_selection())?
            .reference
            .as_ref()
    }

    fn open_live_record(&mut self) {
        if self.pending.is_none()
            && let Some(marker) = self.live_record_selected().cloned()
        {
            self.read_marker(marker);
        }
    }

    fn read_marker(&mut self, marker: Bookmark) {
        self.start_job(
            None,
            vec![
                "evidence".into(),
                marker.channel.name().into(),
                marker.record.to_string().into(),
                "--expected-at".into(),
                marker.time_utc.into(),
            ],
            "Reading the selected Windows record…",
        );
    }

    fn live_layout(&self, active: bool) {
        show(self.controls.live_info, active);
        show(self.controls.live_list, active);
        let top = if active { 496 } else { 350 };
        unsafe {
            SetWindowPos(
                self.controls.status,
                null_mut(),
                16,
                top,
                704,
                112,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    fn finish_live(&mut self, message: &str) {
        self.live = None;
        self.live_layout(false);
        show(self.controls.save_bookmark, false);
        show(self.controls.save_search, false);
        self.offer_health_capture();
        enable(self.controls.mode, self.pending.is_none());
        enable(
            self.controls.include_names,
            self.controls.mode() == Mode::LiveHealth,
        );
        text(self.controls.run, "Start monitoring");
        text(self.controls.open, "Open report");
        enable(self.controls.open, self.completed.is_some());
        text(self.controls.status, message);
    }
}

fn choose_path(window: HWND, save: bool, capture: bool, baseline: bool) {
    let (html_only, recording, version_json) = STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map_or((false, false, false), |state| {
                let mode = state.controls.mode();
                let html_only = mode.html_only()
                    || (mode == Mode::Compare
                        && (!read(state.controls.capture_file).is_empty()
                            || !read(state.controls.baseline_capture).is_empty()));
                (
                    html_only,
                    matches!(mode, Mode::RecordHealth | Mode::ManageRecording),
                    (!save && mode == Mode::CompareVersions)
                        || (save && mode == Mode::SaveVersions),
                )
            })
    });
    let mut path = vec![0u16; 32768];
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: window,
        lpstrFilter: if recording {
            w!("Parsley background recording\0*.jsonl\0\0")
        } else if version_json {
            w!("Parsley inventory version snapshot\0*.json\0\0")
        } else if capture && save {
            w!("Parsley health capture\0*.json\0\0")
        } else if capture {
            w!("Parsley health capture or recording\0*.json;*.jsonl\0\0")
        } else if save && html_only {
            w!("HTML report\0*.html\0\0")
        } else if save {
            w!("HTML report\0*.html\0CSV export\0*.csv\0JSON export\0*.json\0\0")
        } else {
            w!("Windows event log\0*.evtx\0\0")
        },
        lpstrFile: path.as_mut_ptr(),
        nMaxFile: path.len() as u32,
        nFilterIndex: 1,
        lpstrDefExt: if recording {
            w!("jsonl")
        } else if version_json || capture {
            w!("json")
        } else if save {
            null()
        } else {
            w!("evtx")
        },
        Flags: OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | if save { 0 } else { OFN_FILEMUSTEXIST },
        ..Default::default()
    };
    let success = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if success != 0 {
        let length = path
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(path.len());
        let mut selected = PathBuf::from(OsString::from_wide(&path[..length]));
        if save && selected.extension().is_none() {
            selected.set_extension(if recording {
                "jsonl"
            } else if capture || version_json {
                "json"
            } else {
                match dialog.nFilterIndex {
                    2 => "csv",
                    3 => "json",
                    _ => "html",
                }
            });
        }
        STATE.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                if capture && save {
                    state.save_health(&selected);
                    return;
                }
                let control = if capture && baseline {
                    state.controls.baseline_capture
                } else if capture && version_json {
                    state.controls.after_versions
                } else if capture {
                    state.controls.capture_file
                } else if save {
                    state.controls.output
                } else if recording {
                    state.controls.recording_file
                } else if version_json {
                    state.controls.before_versions
                } else {
                    state.controls.source
                };
                let selected = wide(selected.as_os_str());
                unsafe { SetWindowTextW(control, selected.as_ptr()) };
            }
        });
    }
}

fn control(
    parent: HWND,
    class: *const u16,
    name: &str,
    style: u32,
    id: u16,
    bounds: (i32, i32, i32, i32),
) -> Result<HWND, String> {
    let name = wide(name.as_ref());
    let (x, y, width, height) = bounds;
    unsafe {
        let control = CreateWindowExW(
            0,
            class,
            name.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            width,
            height,
            parent,
            id as usize as _,
            GetModuleHandleW(null()),
            null_mut(),
        );
        if control.is_null() {
            return Err("Could not create a Parsley control.".into());
        }
        SendMessageW(
            control,
            WM_SETFONT,
            GetStockObject(DEFAULT_GUI_FONT) as usize,
            0,
        );
        Ok(control)
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn text(control: HWND, value: &str) {
    let value = value.replace('\n', "\r\n");
    let value = wide(value.as_ref());
    unsafe { SetWindowTextW(control, value.as_ptr()) };
}

fn read(control: HWND) -> OsString {
    let length = unsafe { GetWindowTextLengthW(control) };
    let mut value = vec![0u16; length as usize + 1];
    let read = unsafe { GetWindowTextW(control, value.as_mut_ptr(), length + 1) };
    OsString::from_wide(&value[..read as usize])
}

fn enable(control: HWND, enabled: bool) {
    unsafe { EnableWindow(control, enabled.into()) };
}

fn show(control: HWND, wanted: bool) {
    unsafe { ShowWindow(control, if wanted { SW_SHOW } else { SW_HIDE }) };
}

fn checked(control: HWND) -> bool {
    unsafe { SendMessageW(control, BC_GET_CHECK, 0, 0) == CHECKED as isize }
}

fn smoke_controls(output: Option<PathBuf>) -> Result<(), String> {
    STATE.with(|cell| -> Result<(), String> {
        let mut state = cell.borrow_mut();
        let state = state.as_mut().ok_or("Missing desktop state.")?;
        let (sender, receiver) = mpsc::channel();
        state.live = Some(receiver);
        let marker = Bookmark {
            channel: crate::saved::Channel::System,
            record: 42,
            time_utc: "2026-11-22T06:00:00+00:00".into(),
            title: "Fictional restart".into(),
        };
        state.live_lines.push_back(crate::live::Line {
            text: "Fictional restart · System record 42".into(),
            reference: Some(marker.clone()),
        });
        unsafe {
            SendMessageW(
                state.controls.mode,
                CB_SETCURSEL,
                Mode::LiveTimeline.index(),
                0,
            )
        };
        state.render_live_rows(&[]);
        if state.live_record_selected() != Some(&marker) {
            return Err("The live event selection lost its Windows record reference.".into());
        }
        if output.is_some() {
            state.save_bookmark();
            text(state.controls.at, "Fictional");
            state.save_search();
        }
        state.live = None;
        drop(sender);
        if output.is_some() {
            unsafe {
                SendMessageW(
                    state.controls.mode,
                    CB_SETCURSEL,
                    Mode::Bookmarks.index(),
                    0,
                )
            };
            state.change_mode();
            if state.saved.bookmarks != [marker] || state.saved.searches != ["Fictional"] {
                return Err("The desktop could not reopen locally saved items.".into());
            }
            unsafe { SendMessageW(state.controls.mode, CB_SETCURSEL, Mode::Searches.index(), 0) };
            state.change_mode();
            state.start();
            if state.controls.mode() != Mode::LiveTimeline || read(state.controls.at) != "Fictional"
            {
                return Err("The desktop could not apply the saved search.".into());
            }
            text(state.controls.at, "");
        }
        unsafe {
            SendMessageW(
                state.controls.mode,
                CB_SETCURSEL,
                Mode::CompareVersions.index(),
                0,
            )
        };
        state.controls.update_mode();
        text(state.controls.before_versions, "before.json");
        text(state.controls.after_versions, "after.json");
        text(state.controls.output, "version-comparison.html");
        let (_, args) = state.controls.request()?;
        if args.len() != 4
            || args[0] != "inventory-compare"
            || args[1] != "before.json"
            || args[2] != "after.json"
        {
            return Err("The desktop did not preserve the two inventory comparison inputs.".into());
        }
        for (mode, command) in [
            (Mode::Bandwidth, "bandwidth"),
            (Mode::DemoBandwidth, "demo-bandwidth"),
        ] {
            unsafe { SendMessageW(state.controls.mode, CB_SETCURSEL, mode.index(), 0) };
            state.controls.update_mode();
            text(state.controls.output, "bandwidth.html");
            let (path, args) = state.controls.request()?;
            let path = path.ok_or("The bandwidth action did not return its report path.")?;
            if args.len() != 2
                || args[0] != command
                || args[1] != path.as_os_str()
                || path.file_name().is_none_or(|name| name != "bandwidth.html")
            {
                return Err("The desktop did not route the bandwidth action.".into());
            }
        }
        if let Some(output) = &output {
            let path = output.with_extension("health.json");
            state.health_samples.push_back(crate::capture::Sample {
                unix_ms: 1_795_326_000_000,
                cpu_percent: Some(50.0),
                ram_percent: None,
                gpu_peak_percent: None,
                disk_mib_per_sec: None,
                network_kib_per_sec: None,
            });
            state.save_health(&path);
            if crate::capture::Capture::read(&path)?.sample_count() != 1 {
                return Err("The desktop did not save the synthetic health sample.".into());
            }
        }
        unsafe {
            SendMessageW(
                state.controls.mode,
                CB_SETCURSEL,
                Mode::DemoSupport.index(),
                0,
            )
        };
        state.controls.update_mode();
        unsafe { SendMessageW(state.controls.include_inventory, BC_SET_CHECK, CHECKED, 0) };
        text(state.controls.output, "smoke-support.html");
        let (_, args) = state.controls.request()?;
        if !args.iter().any(|arg| arg == "--include-inventory")
            || args.iter().any(|arg| arg == "--include-health")
        {
            return Err(
                "The desktop did not preserve the selected support report sections.".into(),
            );
        }
        let controls = &state.controls;
        unsafe { SendMessageW(controls.mode, CB_SETCURSEL, Mode::DemoCompare.index(), 0) };
        controls.update_mode();
        let path = output
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("smoke-compare.html"));
        let path = wide(path.as_os_str());
        unsafe { SetWindowTextW(controls.output, path.as_ptr()) };
        let (_, args) = controls.request()?;
        if !args.first().is_some_and(|arg| arg == "demo-compare") {
            return Err("The desktop action did not route to comparison.".into());
        }
        if output.is_some() {
            state.start();
        }
        Ok(())
    })?;
    let Some(output) = output else { return Ok(()) };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let complete = STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            let state = state
                .as_mut()
                .expect("desktop state exists through smoke check");
            state.poll();
            state.pending.is_none() && state.completed.is_some()
        });
        if complete {
            let html = std::fs::read_to_string(output).map_err(|error| error.to_string())?;
            if html.contains("Session comparison")
                && html.contains("demo-editor.exe")
                && html.contains("Captured system health")
            {
                return Ok(());
            }
            return Err("The desktop generated the wrong report.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err("The desktop report did not complete.".into())
}
