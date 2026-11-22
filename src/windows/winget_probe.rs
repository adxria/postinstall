use super::deployment::is_elevated;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::ptr::null_mut;
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, WaitForSingleObject};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_millis(400);

pub(super) fn version() -> io::Result<String> {
    probe(0)
}

pub(super) fn version_after_registration() -> io::Result<String> {
    probe(4)
}

fn probe(retries: usize) -> io::Result<String> {
    let elevated = is_elevated().map_err(|error| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("Could not verify this process's privileges. {error}"),
        )
    })?;
    if elevated {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            concat!(
                "Reopen ",
                env!("CARGO_PKG_NAME"),
                " without administrator privileges to check WinGet."
            ),
        ));
    }

    let local_app_data = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        io::Error::other("Windows did not provide a local app data folder for this account.")
    })?;
    let alias = PathBuf::from(local_app_data).join("Microsoft/WindowsApps/winget.exe");
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut command = Command::new(alias);
    command.arg("--version");
    let mut result = run_probe(&mut command, deadline);
    for _ in 0..retries {
        if !matches!(&result, Err(error) if error.kind() != io::ErrorKind::TimedOut)
            || deadline.saturating_duration_since(Instant::now()) <= RETRY_DELAY
        {
            break;
        }
        thread::sleep(RETRY_DELAY);
        result = run_probe(&mut command, deadline);
    }
    result
}

fn run_probe(command: &mut Command, deadline: Instant) -> io::Result<String> {
    let mut child = command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            io::Error::other(format!(
                "Could not run the WinGet app execution alias. {error}"
            ))
        })?;
    wait_for_exit(&mut child, deadline)?;
    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::other(format!("WinGet stopped with {status}.")));
    }

    let stdout = child.stdout.as_ref().expect("probe stdout is piped");
    let mut output = [0u8; 256];
    let mut bytes_read = 0;
    // Read buffered bytes without waiting for EOF. Another process could still hold the write end.
    if unsafe {
        PeekNamedPipe(
            stdout.as_raw_handle(),
            output.as_mut_ptr().cast(),
            output.len() as u32,
            &mut bytes_read,
            null_mut(),
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::other(format!(
            "Could not read the WinGet version. {}",
            io::Error::last_os_error()
        )));
    }
    let version = String::from_utf8_lossy(&output[..bytes_read as usize]);
    let version = version.trim();
    if bytes_read as usize == output.len() || !version.starts_with('v') {
        return Err(io::Error::other("WinGet did not return a version."));
    }
    Ok(version.to_owned())
}

fn wait_for_exit(child: &mut Child, deadline: Instant) -> io::Result<()> {
    let remaining_ms = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as u32;
    // The child owns the process handle for the duration of the bounded wait.
    let outcome = unsafe { WaitForSingleObject(child.as_raw_handle(), remaining_ms) };
    let error = match outcome {
        WAIT_OBJECT_0 => return Ok(()),
        WAIT_TIMEOUT => io::Error::new(
            io::ErrorKind::TimedOut,
            "The WinGet version check did not respond within five seconds.",
        ),
        _ => io::Error::other(format!(
            "Could not wait for the WinGet version check. {}",
            io::Error::last_os_error()
        )),
    };
    let _ = child.kill();
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::system_directory;

    #[test]
    fn reads_version_output_from_an_exited_process() {
        let mut command = Command::new(system_directory().unwrap().join("cmd.exe"));
        command.args(["/D", "/C", "echo v1.2.3"]);
        assert_eq!(
            run_probe(&mut command, Instant::now() + PROBE_TIMEOUT).unwrap(),
            "v1.2.3"
        );
    }

    #[test]
    fn a_hung_probe_is_terminated_after_the_deadline() {
        let powershell = system_directory()
            .unwrap()
            .join("WindowsPowerShell/v1.0/powershell.exe");
        let mut child = Command::new(powershell)
            .creation_flags(CREATE_NO_WINDOW)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .spawn()
            .unwrap();
        let wait = wait_for_exit(&mut child, Instant::now() + Duration::from_millis(100));
        assert_eq!(wait.unwrap_err().kind(), io::ErrorKind::TimedOut);
        // Termination is asynchronous, so give Windows time to signal the process handle.
        assert_eq!(
            unsafe { WaitForSingleObject(child.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0
        );
    }
}
