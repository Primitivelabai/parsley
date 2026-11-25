use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::num::NonZeroIsize;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::ExitCode;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::{Controls::Dialogs::*, Shell::ShellExecuteW, WindowsAndMessaging::*};
use windows_sys::w;
use wry::http::{Method, Request, Response};
use wry::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use wry::{NewWindowResponse, WebContext, WebViewBuilder, WebViewBuilderExtWindows};

const URL: &str = "http://parsley.app/";
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self'; frame-src about:; base-uri 'none'; form-action 'none'";

struct Host(HWND);

impl Drop for Host {
    fn drop(&mut self) {
        if unsafe { IsWindow(self.0) } != 0 {
            unsafe { DestroyWindow(self.0) };
        }
    }
}

impl HasWindowHandle for Host {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        let handle = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}

pub fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next();
    let fallback_smoke = mode.as_deref() == Some(OsStr::new("--smoke-fallback"));
    let smoke = fallback_smoke || mode.as_deref() == Some(OsStr::new("--smoke-ui"));
    if mode.is_some() && !smoke {
        return crate::desktop::main();
    }
    let marker = if smoke {
        args.next().map(PathBuf::from)
    } else {
        None
    };
    if smoke && marker.is_none() {
        eprintln!("Parsley interface smoke requires a receipt path.");
        return ExitCode::FAILURE;
    }
    match desktop(marker.as_deref()) {
        Ok(()) if fallback_smoke => {
            eprintln!("WebView2 was expected to fail before testing the classic fallback.");
            ExitCode::FAILURE
        }
        Ok(()) => ExitCode::SUCCESS,
        Err(_) if fallback_smoke => {
            let mut message = MSG::default();
            if unsafe { PeekMessageW(&mut message, null_mut(), WM_QUIT, WM_QUIT, PM_NOREMOVE) } != 0
            {
                eprintln!("WebView2 startup left a quit message for the classic window.");
                return ExitCode::FAILURE;
            }
            match crate::desktop::smoke(marker.expect("smoke receipt was checked")) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("Parsley classic fallback smoke: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            if smoke {
                eprintln!("Parsley interface smoke: {error}");
                return ExitCode::FAILURE;
            }
            // The portable native interface is available when WebView2 cannot start.
            crate::desktop::main()
        }
    }
}

fn desktop(smoke: Option<&Path>) -> Result<(), String> {
    let profile = if smoke.is_some() {
        tempfile::tempdir().map_err(|error| error.to_string())?
    } else {
        let base = std::env::var_os("LOCALAPPDATA").ok_or("Could not locate local app data.")?;
        let path = PathBuf::from(base).join("Parsley").join("WebView");
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        return host(path, None);
    };
    host(profile.path().to_owned(), smoke)
}

fn host(profile: PathBuf, smoke: Option<&Path>) -> Result<(), String> {
    let instance = unsafe { GetModuleHandleW(null()) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: window_icon(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        lpszClassName: w!("ParsleyInterface"),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err("Could not register the Parsley window.".into());
    }
    let host = Host(unsafe {
        CreateWindowExW(
            0,
            class.lpszClassName,
            w!("Parsley"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1160,
            780,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        )
    });
    if host.0.is_null() {
        return Err("Could not open the Parsley window.".into());
    }
    let hwnd = host.0;
    let (reply, inbox) = mpsc::channel::<(u64, Result<Value, String>)>();
    let state = Arc::new(Mutex::new(Health::default()));
    let capturing = Arc::new(AtomicBool::new(false));
    let (commands, queue) = mpsc::sync_channel::<Packet>(4);
    let job_state = Arc::clone(&state);
    let job_reply = reply.clone();
    std::thread::Builder::new()
        .name("parsley-interface".into())
        .spawn(move || {
            for packet in queue {
                let _ = job_reply.send((packet.id, dispatch(&job_state, &packet)));
            }
        })
        .map_err(|error| error.to_string())?;
    let mut context = WebContext::new(Some(profile));
    let builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("parsley".into(), |_, request| assets(request))
        .with_navigation_handler(|url| url == URL || url == "about:srcdoc" || url == "about:blank")
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_download_started_handler(|_, _| false)
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(false)
        .with_devtools(false)
        .with_visible(smoke.is_none())
        .with_initialization_script(if smoke.is_some() {
            "window.parsleySmoke = true;"
        } else {
            ""
        })
        .with_ipc_handler(move |request| {
            if request.uri() != URL || request.body().len() > 65_536 {
                return;
            }
            let Ok(packet) = serde_json::from_str::<Packet>(request.body()) else {
                return;
            };
            if packet.action == "pick" {
                let _ = reply.send((packet.id, pick(hwnd, packet.string("kind"))));
            } else if packet.action == "health-poll" {
                let _ = reply.send((packet.id, health_poll(&state)));
            } else if matches!(packet.action.as_str(), "bandwidth" | "observe-connections") {
                let id = packet.id;
                if capturing.swap(true, Ordering::AcqRel) {
                    let _ =
                        reply.send((id, Err("A network measurement is already running.".into())));
                    return;
                }
                let capture_state = Arc::clone(&state);
                let active = Arc::clone(&capturing);
                let result = reply.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("parsley-network-view".into())
                    .spawn(move || {
                        let data = dispatch(&capture_state, &packet);
                        active.store(false, Ordering::Release);
                        let _ = result.send((id, data));
                    })
                {
                    capturing.store(false, Ordering::Release);
                    let _ = reply.send((id, Err(error.to_string())));
                }
            } else {
                let id = packet.id;
                if commands.try_send(packet).is_ok() {
                    return;
                }
                let _ = reply.send((id, Err("Parsley is busy. Try again in a moment.".into())));
            }
        })
        .with_url("parsley://app/");
    let webview = builder.build(&host).map_err(|error| error.to_string())?;
    verify_viewport(hwnd, &webview)?;
    if smoke.is_some() {
        if unsafe {
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                930,
                680,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
            )
        } == 0
        {
            return Err("Could not resize the smoke window.".into());
        }
        verify_viewport(hwnd, &webview)?;
    }
    if unsafe { SetTimer(hwnd, 1, 50, None) } == 0 {
        return Err("Could not start the desktop timer.".into());
    }
    // Startup failures may destroy the host before the message loop exists.
    // Post WM_QUIT only for a window whose loop has actually started.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1) };
    if smoke.is_none() {
        unsafe { ShowWindow(hwnd, SW_SHOW) };
    }
    let start = Instant::now();
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result <= 0 {
            if unsafe { IsWindow(hwnd) } != 0 {
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            }
            return if result == 0 {
                Ok(())
            } else {
                Err("Windows stopped the message loop.".into())
            };
        }
        if message.message == WM_TIMER && message.hwnd == hwnd {
            if smoke.is_some() && start.elapsed() > Duration::from_secs(20) {
                return Err("The desktop did not finish its screen checks.".into());
            }
            while let Ok((id, reply)) = inbox.try_recv() {
                if let Some(path) = smoke
                    && id == 0
                {
                    let value = reply?;
                    crate::write_report(path, value.as_str().ok_or("Missing screen receipt.")?)?;
                    return Ok(());
                }
                let value = match reply {
                    Ok(value) => json!({"id": id, "value": value}),
                    Err(error) => json!({"id": id, "error": error}),
                };
                let _ = webview.evaluate_script(&format!("window.parsleyReply({value})"));
            }
        } else {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

fn verify_viewport(hwnd: HWND, webview: &wry::WebView) -> Result<(), String> {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
        return Err("Could not read the desktop viewport.".into());
    }
    let size = webview
        .bounds()
        .map_err(|error| error.to_string())?
        .size
        .to_physical::<i32>(1.0);
    if size.width != rect.right || size.height != rect.bottom || size.width <= 0 || size.height <= 0
    {
        return Err("The Parsley view did not fill the desktop window.".into());
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_DESTROY && unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } != 0 {
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn window_icon() -> HICON {
    let ico = include_bytes!("../ui/parsley.ico");
    // ICO directory entries contain offsets to the individual Windows image resources.
    let Some(entry) = ico.get(6..).and_then(|entries| {
        entries
            .as_chunks::<16>()
            .0
            .iter()
            .take(7)
            .find(|entry| entry[0] == 32)
    }) else {
        return null_mut();
    };
    let length = u32::from_le_bytes(entry[8..12].try_into().unwrap_or_default());
    let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap_or_default()) as usize;
    ico.get(offset..offset.saturating_add(length as usize))
        .map_or(null_mut(), |image| unsafe {
            CreateIconFromResourceEx(image.as_ptr(), length, 1, 0x0003_0000, 32, 32, 0)
        })
}

fn assets(request: Request<Vec<u8>>) -> Response<std::borrow::Cow<'static, [u8]>> {
    let asset: Option<(&str, &[u8])> = if request.method() == Method::GET {
        match request.uri().path() {
            "/" => Some((
                "text/html; charset=utf-8",
                include_bytes!("../ui/index.html"),
            )),
            "/app.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("../ui/app.js"),
            )),
            "/style.css" => Some(("text/css; charset=utf-8", include_bytes!("../ui/style.css"))),
            "/tokens.css" => Some((
                "text/css; charset=utf-8",
                include_bytes!("../ui/tokens.css"),
            )),
            "/sprig.svg" => Some(("image/svg+xml", include_bytes!("../ui/sprig.svg"))),
            "/bunch.svg" => Some(("image/svg+xml", include_bytes!("../ui/bunch.svg"))),
            "/geist.woff2" => Some(("font/woff2", include_bytes!("../ui/geist.woff2"))),
            "/mono.woff2" => Some(("font/woff2", include_bytes!("../ui/mono.woff2"))),
            _ => None,
        }
    } else {
        None
    };
    let Some((content_type, data)) = asset else {
        let mut response = Response::new(std::borrow::Cow::Borrowed(&[][..]));
        *response.status_mut() = wry::http::StatusCode::NOT_FOUND;
        return response;
    };
    Response::builder()
        .header("Content-Type", content_type)
        .header("Content-Security-Policy", CSP)
        .header("X-Content-Type-Options", "nosniff")
        .body(std::borrow::Cow::Borrowed(data))
        .expect("static headers")
}

#[derive(Deserialize)]
struct Packet {
    id: u64,
    action: String,
    #[serde(default)]
    data: Value,
}

impl Packet {
    fn string(&self, key: &str) -> &str {
        self.data[key].as_str().unwrap_or("")
    }

    fn flag(&self, key: &str) -> bool {
        self.data[key].as_bool().unwrap_or(false)
    }
}

fn pick(hwnd: HWND, kind: &str) -> Result<Value, String> {
    let (save, filter, extension) = match kind {
        "event" => (false, w!("Windows event log\0*.evtx\0\0"), w!("evtx")),
        "health" => (
            false,
            w!("Health capture or journal\0*.json;*.jsonl\0\0"),
            null(),
        ),
        "version" => (false, w!("Version snapshot\0*.json\0\0"), w!("json")),
        "journal" => (false, w!("Health journal\0*.jsonl\0\0"), w!("jsonl")),
        "open-report" => (false, w!("HTML report\0*.html;*.htm\0\0"), w!("html")),
        "html" => (true, w!("HTML report\0*.html\0\0"), w!("html")),
        "csv" => (true, w!("CSV report\0*.csv\0\0"), w!("csv")),
        "json" => (true, w!("JSON file\0*.json\0\0"), w!("json")),
        "case" => (false, w!("Incident case\0*.json\0\0"), w!("json")),
        "debugger" => (false, w!("WinDbg text report\0*.txt;*.log\0\0"), w!("txt")),
        "jsonl" => (true, w!("Health journal\0*.jsonl\0\0"), w!("jsonl")),
        _ => return Err("Choose a supported file type.".into()),
    };
    let mut path = vec![0; 32768];
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: filter,
        lpstrDefExt: extension,
        lpstrFile: path.as_mut_ptr(),
        nMaxFile: path.len() as u32,
        Flags: OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | if save { 0 } else { OFN_FILEMUSTEXIST },
        ..Default::default()
    };
    let selected = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if selected == 0 {
        return if unsafe { CommDlgExtendedError() } == 0 {
            Ok(Value::Null)
        } else {
            Err("Windows could not open the file picker.".into())
        };
    }
    let end = path
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(path.len());
    let path = String::from_utf16(&path[..end]).map_err(|_| "The filename was invalid.")?;
    validate_local_path(&path)?;
    if save
        && Path::new(&path)
            .try_exists()
            .map_err(|error| error.to_string())?
    {
        return Err("Choose a new file. Existing documents are kept.".into());
    }
    Ok(json!(path))
}

fn validate_local_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || (Path::new(path).is_absolute()
            && matches!(Path::new(path).components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))))
    {
        Ok(())
    } else {
        Err("Choose a file on a Windows drive.".into())
    }
}

fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
    values.iter().map(OsString::from)
}

fn open_report(path: &str) -> Result<Value, String> {
    let file = Path::new(path);
    if !file.is_absolute()
        || !file.is_file()
        || !file
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"))
    {
        return Err("Choose an existing local HTML report.".into());
    }
    shell_open(file.as_os_str(), "Windows could not open the report.")
}

fn shell_open(target: &OsStr, error: &str) -> Result<Value, String> {
    let target: Vec<_> = target.encode_wide().chain([0]).collect();
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            w!("open"),
            target.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        Err(error.into())
    } else {
        Ok(Value::Null)
    }
}

fn dispatch(state: &Arc<Mutex<Health>>, packet: &Packet) -> Result<Value, String> {
    for key in [
        "path",
        "source",
        "before",
        "after",
        "health",
        "baseHealth",
        "input",
        "debugger",
    ] {
        validate_local_path(packet.string(key))?;
    }
    let demo = packet.flag("demo");
    let path = packet.string("path");
    match packet.action.as_str() {
        "smoke" if packet.id == 0 => Ok(json!(packet.string("receipt"))),
        "smoke-error" if packet.id == 0 => Err(packet.string("receipt").into()),
        "events" => events(packet, false),
        "demo-cases" => Ok(json!(
            crate::incidents::DEMO_CASES
                .iter()
                .map(|case| json!({"id": case.id, "title": case.title}))
                .collect::<Vec<_>>()
        )),
        "history" => events(packet, true),
        "report" => report(packet),
        "system" => crate::snapshot(if demo { "demo-health" } else { "health" })
            .map(|(_, text)| json!(text)),
        "inventory" => crate::snapshot(if demo { "demo-inventory" } else { "inventory" })
            .map(|(_, text)| json!(text)),
        "endpoints" => crate::snapshot(if demo {
            "demo-connections"
        } else {
            "connections"
        })
        .map(|(_, text)| json!(text)),
        "observe-connections" => {
            crate::connections::observe(demo).map(|observation| json!(observation))
        }
        "bandwidth" => {
            let (data, html) = crate::network::interface_capture(demo)?;
            state
                .lock()
                .map_err(|_| "Could not keep the bandwidth report.")?
                .bandwidth_report = Some(html);
            Ok(data)
        }
        "bandwidth-save" => {
            if crate::export::Format::from_path(Path::new(path))? != crate::export::Format::Html {
                return Err("Choose an HTML destination.".into());
            }
            let state = state
                .lock()
                .map_err(|_| "Could not read the bandwidth report.")?;
            let html = state
                .bandwidth_report
                .as_deref()
                .ok_or("Capture bandwidth first.")?;
            crate::write_report(Path::new(path), html)?;
            Ok(json!("Saved the bandwidth report."))
        }
        "open" => open_report(path),
        "open-help" => {
            let article = crate::incidents::official_help(packet.string("topic"))
                .ok_or("Choose a help article from Parsley.")?;
            shell_open(
                OsStr::new(article.url),
                "Windows could not open Microsoft Support.",
            )
        }
        "case-create" | "case-demo" | "case-report" | "case-bookmark" => {
            let values = case_args(packet)?;
            let values: Vec<&str> = values.iter().map(String::as_str).collect();
            crate::run(args(&values)).map(|value| json!(value))
        }
        "evidence" => {
            let mut values = vec![
                "evidence",
                packet.string("channel"),
                packet.string("record"),
            ];
            if !packet.string("expected").is_empty() {
                values.extend(["--expected-at", packet.string("expected")]);
            }
            if !packet.string("source").is_empty() {
                values.extend(["--from", packet.string("source")]);
            }
            crate::run(args(&values)).map(|value| json!(value))
        }
        "saved" => crate::saved::Store::for_user()?
            .read()
            .map(|saved| json!(saved)),
        "search-save" => crate::saved::Store::for_user()?
            .change(|saved| saved.add_search(packet.string("query")))
            .map(|saved| json!(saved)),
        "search-remove" => crate::saved::Store::for_user()?
            .change(|saved| {
                saved
                    .searches
                    .retain(|query| query != packet.string("query"));
                Ok(())
            })
            .map(|saved| json!(saved)),
        "bookmark-save" | "bookmark-remove" => {
            let marker =
                serde_json::from_value::<crate::saved::Bookmark>(packet.data["bookmark"].clone())
                    .map_err(|_| "Choose a Windows record.")?;
            if marker.record == 0
                || marker.title.len() > 150
                || crate::timestamp(&marker.time_utc, "event time").is_err()
            {
                return Err("The Windows record reference is invalid.".into());
            }
            crate::saved::Store::for_user()?
                .change(|saved| {
                    if packet.action == "bookmark-save" {
                        saved.bookmark(marker);
                    } else {
                        saved.remove_bookmark(&marker);
                    }
                    Ok(())
                })
                .map(|saved| json!(saved))
        }
        "health-start" => health_start(state, packet.flag("warn")),
        "memory-now" => {
            let reading = crate::system::ProcessMemory::open()
                .and_then(|counter| counter.read())
                .ok_or("Windows did not provide per-app memory readings.")?;
            let mut memory = crate::system::MemoryHistory::default();
            memory.push(reading);
            Ok(json!(memory.summary()))
        }
        "health-stop" => {
            state
                .lock()
                .map_err(|_| "Health state is unavailable.")?
                .running = false;
            Ok(Value::Null)
        }
        "health-save" => {
            let samples = state
                .lock()
                .map_err(|_| "Health state is unavailable.")?
                .samples
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            if samples.is_empty() {
                return Err("Start monitoring to collect health readings first.".into());
            }
            let capture = crate::capture::Capture::from_samples(samples);
            let data = serde_json::to_string_pretty(&capture).map_err(|error| error.to_string())?;
            if crate::export::Format::from_path(Path::new(path))? != crate::export::Format::Json {
                return Err("Choose a JSON file for the health capture.".into());
            }
            crate::write_report(Path::new(path), &data)?;
            Ok(json!("Saved health readings."))
        }
        "record-start" | "record-status" | "record-stop" => {
            let store = crate::saved::Store::for_user()?;
            let saved_path;
            let file = if path.is_empty() {
                saved_path = store
                    .read()?
                    .last_recording
                    .ok_or("Choose a recording file.")?;
                &saved_path
            } else {
                path
            };
            let command = match packet.action.as_str() {
                "record-start" => "start-recording",
                "record-stop" => "stop-recording",
                _ => "recording-status",
            };
            let options = if command == "start-recording" {
                vec![command, file, "--minutes", packet.string("minutes")]
            } else {
                vec![command, file]
            };
            let status = crate::run(args(&options))?;
            if command == "start-recording" {
                store.change(|saved| {
                    saved.last_recording = Some(file.into());
                    Ok(())
                })?;
            }
            Ok(json!({"status": status, "path": file}))
        }
        _ => Err("Unknown Parsley action.".into()),
    }
}

/// Command-line arguments for the incident-case actions from the desktop.
fn case_args(packet: &Packet) -> Result<Vec<String>, String> {
    let text = |key: &str| packet.string(key).to_owned();
    let path = text("path");
    if path.is_empty() {
        return Err("Choose where to save the file.".into());
    }
    match packet.action.as_str() {
        "case-demo" => Ok(vec!["case-demo".into(), path]),
        "case-report" => Ok(vec!["case-report".into(), text("input"), path]),
        "case-bookmark" => {
            let channel = match packet.string("channel") {
                channel @ ("System" | "Application") => channel,
                _ => return Err("Choose a System or Application record.".into()),
            };
            let record = packet
                .string("record")
                .parse::<u64>()
                .ok()
                .filter(|record| *record > 0)
                .ok_or("Choose a Windows record.")?;
            Ok(vec![
                "case-bookmark".into(),
                text("input"),
                path,
                format!("1:{channel}:{record}"),
            ])
        }
        "case-create" => {
            let mut values = vec!["case-create".into(), path, "--at".into(), text("at")];
            if !packet.string("minutes").is_empty() {
                values.extend(["--minutes".into(), text("minutes")]);
            }
            if !packet.string("source").is_empty() {
                values.extend(["--evtx".into(), text("source")]);
            }
            if !packet.string("debugger").is_empty() {
                values.extend([
                    "--debugger-report".into(),
                    text("debugger"),
                    "--debugger-at".into(),
                    text("debuggerAt"),
                ]);
                if packet.flag("includeModule") {
                    values.push("--include-module".into());
                }
            }
            Ok(values)
        }
        _ => Err("Unknown Parsley action.".into()),
    }
}

fn events(packet: &Packet, history: bool) -> Result<Value, String> {
    let now = DateTime::<Utc>::from(SystemTime::now());
    let demo = packet.flag("demo");
    let source = (!packet.string("source").is_empty())
        .then(|| crate::log_path(PathBuf::from(packet.string("source"))))
        .transpose()?;
    let interval = if history {
        None
    } else if !packet.string("at").is_empty() {
        let minutes = packet
            .string("minutes")
            .parse::<i64>()
            .ok()
            .filter(|minutes| (1..=180).contains(minutes))
            .ok_or("Enter 1–180 minutes.")?;
        Some(crate::window(
            crate::timestamp(packet.string("at"), "incident time")?,
            TimeDelta::minutes(minutes),
        )?)
    } else if source.is_none() {
        Some((
            now - (if packet.flag("live") {
                TimeDelta::minutes(15)
            } else {
                TimeDelta::days(1)
            }),
            now,
        ))
    } else {
        None
    };
    let case = if demo && !history && !packet.string("case").is_empty() {
        Some(crate::incidents::DemoCase::get(packet.string("case"))?)
    } else {
        None
    };
    let days = packet
        .string("days")
        .parse::<i64>()
        .ok()
        .filter(|days| [7, 30, 90].contains(days))
        .unwrap_or(30);
    let (now, mut session) = if history {
        crate::history_session(demo, days)?
    } else if demo {
        (
            now,
            crate::report::Session {
                events: case
                    .map_or_else(crate::incidents::demo, crate::incidents::DemoCase::events),
                notices: vec![],
                window: None,
            },
        )
    } else {
        (now, crate::collect(source.as_deref(), interval)?)
    };
    let mut analysis = Value::Null;
    if history {
        session.events.retain(|event| event.kind.is_incident());
        let health = (!packet.string("health").is_empty())
            .then(|| crate::capture::Capture::read(Path::new(packet.string("health"))))
            .transpose()?;
        let versions = if packet.string("before").is_empty() && packet.string("after").is_empty() {
            None
        } else if packet.string("before").is_empty() || packet.string("after").is_empty() {
            return Err("Choose both inventory snapshots, or neither.".into());
        } else {
            Some((
                crate::versions::Snapshot::read(Path::new(packet.string("before")))?,
                crate::versions::Snapshot::read(Path::new(packet.string("after")))?,
            ))
        };
        let clues = crate::crash_groups::Clues {
            health: health.as_ref(),
            versions: versions.as_ref().map(|(before, after)| (before, after)),
        };
        let names = demo || packet.flag("names");
        analysis = json!(crate::crash_groups::analyze(
            &session.events,
            now,
            names,
            clues
        ));
    }
    let query = packet.string("query").trim().to_lowercase();
    let query_parts: Vec<_> = query.split_whitespace().take(12).collect();
    let mut counts = std::collections::BTreeMap::new();
    let matches: Vec<_> = session
        .events
        .iter()
        .rev()
        .filter(|event| {
            let text = format!(
                "{} {} {} {} {} {} {}",
                event.kind.title(),
                event.kind.explanation(),
                event.kind.provider(),
                event.kind.channel(),
                event.id,
                event.record,
                event.detail.application.as_deref().unwrap_or("")
            )
            .to_lowercase();
            query_parts.iter().all(|part| text.contains(part))
        })
        .collect();
    for event in &matches {
        *counts.entry(event.kind.title()).or_insert(0usize) += 1;
    }
    let show_names = !history || demo || packet.flag("names");
    let rows: Vec<_> = matches.iter().take(800).map(|event| json!({
        "at": event.at.to_rfc3339(), "record": event.record.to_string(), "id": event.id,
        "title": event.kind.title(), "provider": event.kind.provider(), "channel": event.kind.channel(),
        "kind": event.kind.key(),
        "explanation": event.kind.explanation(),
        "next_step": event.kind.next_step(),
        "help": event.kind.help(),
        "application": event.detail.application.as_ref().filter(|_| show_names),
        "module": event.detail.module.as_ref().filter(|_| show_names), "stop_code": event.detail.stop_code,
    })).collect();
    Ok(
        json!({"rows": rows, "count": matches.len(), "notices": session.notices, "counts": counts, "analysis": analysis,
        "case": case.map(|case| json!({"title": case.title, "summary": case.summary, "next": case.next, "evidence_event_id": case.evidence.demo_id()}))}),
    )
}

fn report(packet: &Packet) -> Result<Value, String> {
    let demo = packet.flag("demo");
    let kind = packet.string("kind");
    let command = match (kind, demo) {
        ("compare", true) => "demo-compare",
        ("compare", false) => "compare",
        ("inventory-compare", true) => "demo-inventory-compare",
        ("inventory-compare", false) => "inventory-compare",
        ("inventory", true) => "demo-inventory",
        ("inventory", false) => "inventory",
        ("inventory-save", false) => "inventory-save",
        ("support", true) => "demo-support",
        ("support", false) => "support",
        ("history", true) => "demo-history",
        ("history", false) => "history",
        ("timeline", true) if !packet.string("case").is_empty() => "demo-case",
        ("timeline", true) => "demo",
        ("timeline", false) if !packet.string("source").is_empty() => "import",
        ("timeline", false) => "scan",
        _ => return Err("Choose an available report.".into()),
    };
    let destination = packet.string("path");
    let (temp, output) = if destination.is_empty() {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let output = temp.path().join("report.html");
        (Some(temp), output)
    } else {
        (None, PathBuf::from(destination))
    };
    let mut values = vec![command];
    if command == "demo-case" {
        values.push(packet.string("case"));
    }
    if command == "import" {
        values.push(packet.string("source"));
    }
    if command == "inventory-compare" {
        values.extend([packet.string("before"), packet.string("after")]);
    }
    values.push(output.to_str().ok_or("The report path is invalid.")?);
    if command == "compare" {
        values.extend([
            "--at",
            packet.string("at"),
            "--baseline",
            packet.string("baseline"),
            "--minutes",
            packet.string("minutes"),
        ]);
        for (key, option) in [
            ("source", "--from"),
            ("health", "--health"),
            ("baseHealth", "--baseline-health"),
        ] {
            if !packet.string(key).is_empty() {
                values.extend([option, packet.string(key)]);
            }
        }
    }
    if matches!(command, "scan" | "import") && !packet.string("at").is_empty() {
        values.extend([
            "--at",
            packet.string("at"),
            "--minutes",
            packet.string("minutes"),
        ]);
    }
    if command == "history" {
        values.extend(["--days", packet.string("days")]);
        if !packet.string("health").is_empty() {
            values.extend(["--health", packet.string("health")]);
        }
        if !packet.string("before").is_empty() || !packet.string("after").is_empty() {
            values.extend([
                "--versions",
                packet.string("before"),
                packet.string("after"),
            ]);
        }
    }
    if command == "demo-history" && !packet.string("days").is_empty() {
        values.extend(["--days", packet.string("days")]);
    }
    if matches!(command, "scan" | "import" | "history" | "compare") && packet.flag("names") {
        values.push("--include-app-names");
    }
    if kind == "support" {
        if packet.flag("health") {
            values.push("--include-health");
        }
        if packet.flag("inventory") {
            values.push("--include-inventory");
        }
    }
    let status = crate::run(args(&values))?;
    if temp.is_some() {
        let html = std::fs::read_to_string(output).map_err(|error| error.to_string())?;
        Ok(json!({"status": status, "html": html}))
    } else {
        Ok(json!({"status": status, "path": output}))
    }
}

#[derive(Default)]
struct Health {
    bandwidth_report: Option<String>,
    generation: u64,
    running: bool,
    error: Option<String>,
    samples: VecDeque<crate::capture::Sample>,
    latest: Vec<String>,
    memory: Option<crate::system::MemorySummary>,
}

fn health_poll(state: &Mutex<Health>) -> Result<Value, String> {
    let state = state.lock().map_err(|_| "Health state is unavailable.")?;
    let chart: Vec<_> = state.samples.iter().rev().take(60).rev().collect();
    Ok(
        json!({"running": state.running, "error": state.error, "chart": chart, "latest": state.latest, "memory": state.memory}),
    )
}

fn health_start(state: &Arc<Mutex<Health>>, warn: bool) -> Result<Value, String> {
    let mut health = state.lock().map_err(|_| "Health state is unavailable.")?;
    if health.running {
        return Ok(Value::Null);
    }
    *health = Health {
        bandwidth_report: health.bandwidth_report.take(),
        generation: health.generation + 1,
        running: true,
        ..Default::default()
    };
    let generation = health.generation;
    let state = Arc::clone(state);
    let worker = std::thread::Builder::new()
        .name("parsley-health-view".into())
        .spawn(move || {
            let memory = crate::system::ProcessMemory::open();
            let mut history = crate::system::MemoryHistory::default();
            let mut last_memory_read = None::<std::time::Instant>;
            let result = crate::system::watch_health(warn, |batch| {
                let reading = if let Some(counter) = &memory
                    && last_memory_read.is_none_or(|last| last.elapsed().as_secs() >= 5)
                {
                    last_memory_read = Some(std::time::Instant::now());
                    counter.read()
                } else {
                    None
                };
                let Ok(mut health) = state.lock() else {
                    return false;
                };
                if !health.running || health.generation != generation {
                    return false;
                }
                if let Some(reading) = reading {
                    history.push(reading);
                    health.memory = history.summary();
                }
                if let Some(sample) = batch.health {
                    if health.samples.len() == crate::capture::MAX_SAMPLES {
                        health.samples.pop_front();
                    }
                    health.samples.push_back(sample);
                }
                health.latest = batch
                    .lines
                    .into_iter()
                    .map(|line| line.text)
                    .chain(batch.notices)
                    .collect();
                true
            });
            if let Ok(mut health) = state.lock()
                && health.generation == generation
            {
                health.running = false;
                health.error = result.err();
            }
        });
    if let Err(error) = worker {
        health.running = false;
        return Err(error.to_string());
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::{Packet, case_args, validate_local_path};
    use serde_json::json;

    #[test]
    fn help_opener_only_accepts_curated_topic_ids() {
        assert!(
            crate::incidents::official_help("restart")
                .unwrap()
                .url
                .starts_with("https://support.microsoft.com/")
        );
        assert!(crate::incidents::official_help("https://support.microsoft.com/").is_none());
        assert!(crate::incidents::official_help("file:///C:/some-local-file").is_none());
    }

    fn packet(action: &str, data: serde_json::Value) -> Packet {
        Packet {
            id: 1,
            action: action.into(),
            data,
        }
    }

    #[test]
    #[ignore = "writes fictional preview fixtures for screenshots"]
    fn dump_preview_fixtures() {
        let path = std::env::var("PARSLEY_FIXTURES").expect("set PARSLEY_FIXTURES");
        let state = std::sync::Arc::new(std::sync::Mutex::new(super::Health::default()));
        let mut calls = vec![
            ("demo-cases", json!({})),
            ("events", json!({"demo": true})),
            ("events", json!({"demo": true, "query": "restart"})),
            ("system", json!({"demo": true})),
            ("inventory", json!({"demo": true})),
            ("endpoints", json!({"demo": true})),
            ("bandwidth", json!({"demo": true})),
        ];
        for case in ["graphics", "application", "restart"] {
            calls.push(("events", json!({"demo": true, "case": case})));
        }
        for days in ["7", "30", "90"] {
            calls.push(("history", json!({"demo": true, "days": days})));
        }
        for kind in ["compare", "inventory-compare", "support", "timeline"] {
            calls.push(("report", json!({"demo": true, "kind": kind})));
        }
        let mut out = serde_json::Map::new();
        for (action, data) in calls {
            let key = format!("{action} {data}");
            let value = match super::dispatch(&state, &packet(action, data)) {
                Ok(value) => json!({"value": value}),
                Err(error) => json!({"error": error}),
            };
            out.insert(key, value);
        }
        std::fs::write(path, serde_json::Value::Object(out).to_string()).unwrap();
    }

    #[test]
    fn case_actions_build_the_command_line() {
        let create = packet(
            "case-create",
            json!({"path": r"C:\Cases\new.json", "at": "2026-11-22T06:14:00Z", "minutes": "10",
                "source": r"C:\Logs\System.evtx", "debugger": r"C:\Logs\analysis.txt",
                "debuggerAt": "2026-11-22T06:15:00Z", "includeModule": true}),
        );
        assert_eq!(
            case_args(&create).unwrap(),
            [
                "case-create",
                r"C:\Cases\new.json",
                "--at",
                "2026-11-22T06:14:00Z",
                "--minutes",
                "10",
                "--evtx",
                r"C:\Logs\System.evtx",
                "--debugger-report",
                r"C:\Logs\analysis.txt",
                "--debugger-at",
                "2026-11-22T06:15:00Z",
                "--include-module",
            ]
        );
        let bookmark = packet(
            "case-bookmark",
            json!({"path": r"C:\Cases\b.json", "input": r"C:\Cases\a.json", "channel": "System", "record": "42"}),
        );
        assert_eq!(
            case_args(&bookmark).unwrap(),
            [
                "case-bookmark",
                r"C:\Cases\a.json",
                r"C:\Cases\b.json",
                "1:System:42"
            ]
        );
        for data in [
            json!({"path": r"C:\b.json", "input": r"C:\a.json", "channel": "Security", "record": "42"}),
            json!({"path": r"C:\b.json", "input": r"C:\a.json", "channel": "System", "record": "0"}),
            json!({"path": r"C:\b.json", "input": r"C:\a.json", "channel": "System", "record": "1:System:2"}),
        ] {
            assert!(case_args(&packet("case-bookmark", data)).is_err());
        }
        assert!(case_args(&packet("case-report", json!({"input": r"C:\a.json"}))).is_err());
    }

    #[test]
    fn desktop_file_fields_accept_drive_paths_and_reject_network_and_device_paths() {
        assert!(validate_local_path(r"C:\Windows\Example.evtx").is_ok());
        assert!(validate_local_path("").is_ok());
        for path in [
            r"\\server\share\example.evtx",
            r"\\.\pipe\report",
            r"C:relative.html",
            "example.evtx",
        ] {
            assert!(validate_local_path(path).is_err(), "{path}");
        }
    }
}
