#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    parsley::interface::main()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("Parsley Desktop requires Windows.");
    std::process::ExitCode::FAILURE
}
