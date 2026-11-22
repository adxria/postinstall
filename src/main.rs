#![cfg_attr(windows, windows_subsystem = "windows")]
#![deny(unsafe_code)]

#[cfg(any(windows, test))]
mod installer;
#[cfg(windows)]
mod settings;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(any(windows, test))]
mod winget;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    windows::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Build postinstall for x86_64-pc-windows-msvc to run it on Windows.");
}
