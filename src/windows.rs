use crate::installer::{Installation, ProvisionProgress, Provisioned};
use crate::settings::{RECOMMENDATIONS, Recommendation, Target};
use crate::winget::Result;
use deployment::{EXIT_FAILED, InstallOutcome, ProvisionReport, WORKER_ARG};
use std::cell::{Cell, RefCell};
use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::process::ExitCode;
use std::ptr::{from_ref, null, null_mut};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemDirectoryW, GetSystemWindowsDirectoryW,
};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::w;

mod deployment;
mod preferences;
mod theme;
mod winget_probe;

const APP_NAME: &str = env!("CARGO_PKG_NAME");
const WINGET_INSTALL_TITLE: &str = concat!(env!("CARGO_PKG_NAME"), " - WinGet installation");

const GUIDE: usize = 100;
const GUIDE_STATUS: i32 = 101;
const INSTALL: i32 = 102;
const SETTINGS_LIST: i32 = 103;
const TITLE: i32 = 104;
const DETAILS: i32 = 105;
const SETTINGS_ACTION: i32 = 106;
const SETTINGS_APPLY: i32 = 108;
const WORKER: usize = 200;
const WORKER_STATUS: i32 = 201;
const PROGRESS: i32 = 202;
const WORKER_DETAILS: i32 = 203;
const CLOSE: i32 = 204;
const JOB_TIMER: usize = 1;
const GUIDE_BUTTONS: &[(i32, i32)] = &[
    (INSTALL, BS_PUSHBUTTON),
    (SETTINGS_ACTION, BS_PUSHBUTTON),
    (SETTINGS_APPLY, BS_PUSHBUTTON),
];
const WORKER_BUTTONS: &[(i32, i32)] = &[(CLOSE, BS_DEFPUSHBUTTON)];

enum GuideUpdate {
    Probed(io::Result<String>),
    Registering,
    Finished(InstallOutcome),
}

enum WorkerUpdate {
    Progress(ProvisionProgress),
    Finished(ProvisionReport),
}

struct GuideState {
    job: DialogJob<GuideUpdate>,
    installing: Cell<bool>,
}

struct DialogJob<T> {
    button: i32,
    updates: RefCell<Option<Receiver<T>>>,
}

impl<T: Send + 'static> DialogJob<T> {
    fn new(button: i32) -> Self {
        Self {
            button,
            updates: RefCell::new(None),
        }
    }

    fn is_active(&self) -> bool {
        self.updates.borrow().is_some()
    }

    fn start(&self, window: HWND, task: impl FnOnce(Sender<T>) + Send + 'static) -> io::Result<()> {
        // Windows delivers the timer to this dialog's procedure.
        if unsafe { SetTimer(window, JOB_TIMER, 100, None) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let (sender, receiver) = mpsc::channel();
        if let Err(error) = thread::Builder::new().spawn(move || task(sender)) {
            // The timer was created above and no receiver has been stored.
            unsafe { KillTimer(window, JOB_TIMER) };
            return Err(error);
        }
        self.updates.replace(Some(receiver));
        enable(window, self.button, false);
        Ok(())
    }

    fn next(&self) -> Option<std::result::Result<T, TryRecvError>> {
        self.updates.borrow().as_ref().map(Receiver::try_recv)
    }

    fn finish(&self, window: HWND) {
        self.updates.replace(None);
        // The timer belongs to the current dialog.
        unsafe { KillTimer(window, JOB_TIMER) };
        enable(window, self.button, true);
    }
}

pub fn run() -> ExitCode {
    match show_dialog() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            show_message(null_mut(), APP_NAME, &error.to_string(), MB_ICONERROR);
            ExitCode::FAILURE
        }
    }
}

fn show_dialog() -> Result<u8> {
    let _com = ComApartment::initialize()?;
    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new(WORKER_ARG)) {
        if !deployment::is_elevated()? {
            return Err("WinGet installation requires administrator approval.".into());
        }
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS,
        };
        // Windows reads the correctly sized structure during this call.
        if unsafe { InitCommonControlsEx(&controls) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        show_modal(
            WORKER,
            Some(worker_dialog),
            &DialogJob::<WorkerUpdate>::new(CLOSE),
        )
    } else {
        let state = GuideState {
            job: DialogJob::new(INSTALL),
            installing: Cell::new(false),
        };
        show_modal(GUIDE, Some(guide_dialog), &state)
    }
}

fn show_modal<T>(resource: usize, procedure: DLGPROC, state: &T) -> Result<u8> {
    // The resource ID fits in the low word. The modal call keeps the state borrowed until the dialog is destroyed.
    let result = unsafe {
        DialogBoxParamW(
            null_mut(),
            resource as *const u16,
            null_mut(),
            procedure,
            from_ref(state) as LPARAM,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(u8::try_from(result)?)
}

fn dialog_data<T>(window: HWND, message: u32, lparam: LPARAM) -> *const T {
    // WM_INITDIALOG receives the pointer passed to DialogBoxParamW. Subsequent messages use this dialog's slot.
    unsafe {
        if message == WM_INITDIALOG {
            SetWindowLongPtrW(window, GWLP_USERDATA, lparam);
            return lparam as *const T;
        }
        GetWindowLongPtrW(window, GWLP_USERDATA) as *const T
    }
}

unsafe extern "system" fn guide_dialog(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    // The callback forwards the original, synchronous paint-message arguments.
    if let Some(result) = unsafe { theme::paint(message, wparam, lparam, DETAILS) } {
        return result;
    }
    // show_dialog supplies GuideState for this procedure and keeps it alive through every callback.
    let Some(state) = (unsafe { dialog_data::<GuideState>(window, message, lparam).as_ref() })
    else {
        return 0;
    };
    match message {
        WM_INITDIALOG => {
            theme::refresh(window, GUIDE_BUTTONS, None);
            for entry in RECOMMENDATIONS {
                let title = wide(entry.title);
                // List boxes copy the NUL-terminated label during LB_ADDSTRING.
                unsafe {
                    SendDlgItemMessageW(
                        window,
                        SETTINGS_LIST,
                        LB_ADDSTRING,
                        0,
                        title.as_ptr() as isize,
                    )
                };
            }
            // The first list row exists and the selection message has no pointer argument.
            unsafe { SendDlgItemMessageW(window, SETTINGS_LIST, LB_SETCURSEL, 0, 0) };
            show_selection(window);
            if let Err(error) = state.job.start(window, |updates| {
                let _ = updates.send(GuideUpdate::Probed(winget_probe::version()));
            }) {
                guide_job_error(window, &error.to_string());
            }
        }
        WM_COMMAND => match command_id(wparam) {
            SETTINGS_LIST if (wparam >> 16) as u32 == LBN_SELCHANGE => show_selection(window),
            SETTINGS_ACTION => open_selection(window, false),
            SETTINGS_APPLY => open_selection(window, true),
            INSTALL if !state.job.is_active() => start_install(window, state),
            IDCANCEL => close_guide(window, state),
            _ => return 0,
        },
        WM_TIMER if wparam == JOB_TIMER => guide_updates(window, state),
        WM_CLOSE => close_guide(window, state),
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            theme::refresh(window, GUIDE_BUTTONS, None);
            return 0;
        }
        _ => return 0,
    }
    1
}

fn command_id(wparam: WPARAM) -> i32 {
    (wparam & 0xffff) as i32
}

fn close_guide(window: HWND, state: &GuideState) {
    if !state.installing.get() {
        end_dialog(window, 0);
    }
}

fn start_install(window: HWND, state: &GuideState) {
    state.installing.set(true);
    set_text(window, GUIDE_STATUS, "Waiting for WinGet installation…");
    if let Err(error) = state.job.start(window, |updates| {
        let outcome = deployment::install_for_user(|| {
            let _ = updates.send(GuideUpdate::Registering);
        });
        let _ = updates.send(GuideUpdate::Finished(outcome));
    }) {
        state.installing.set(false);
        guide_job_error(window, &error.to_string());
    }
}

fn guide_updates(window: HWND, state: &GuideState) {
    loop {
        match state.job.next() {
            Some(Ok(GuideUpdate::Registering)) => {
                set_text(
                    window,
                    GUIDE_STATUS,
                    "Registering and checking WinGet for this Windows user…",
                );
            }
            Some(Ok(GuideUpdate::Probed(version))) => {
                state.job.finish(window);
                let status = match version {
                    Ok(version) => format!("WinGet {version} is ready."),
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                        error.to_string()
                    }
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                        "WinGet did not respond. You can use Install or update WinGet.".into()
                    }
                    Err(_) => "WinGet is unavailable for this user.".into(),
                };
                set_text(window, GUIDE_STATUS, &status);
                break;
            }
            Some(Ok(GuideUpdate::Finished(outcome))) => {
                state.job.finish(window);
                state.installing.set(false);
                show_install_outcome(window, outcome);
                break;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                state.job.finish(window);
                state.installing.set(false);
                guide_job_error(window, "The background operation ended unexpectedly.");
                break;
            }
            Some(Err(TryRecvError::Empty)) | None => break,
        }
    }
}

fn guide_job_error(window: HWND, error: &str) {
    set_text(
        window,
        GUIDE_STATUS,
        "The background operation could not finish.",
    );
    show_message(window, APP_NAME, error, MB_ICONERROR);
}

fn show_install_outcome(window: HWND, outcome: InstallOutcome) {
    match outcome {
        InstallOutcome::Canceled => {
            set_text(
                window,
                GUIDE_STATUS,
                "Installation canceled. You can try again.",
            );
        }
        InstallOutcome::LaunchFailed(error) => {
            set_text(
                window,
                GUIDE_STATUS,
                "Could not start the WinGet installer.",
            );
            show_message(
                window,
                WINGET_INSTALL_TITLE,
                &error.to_string(),
                MB_ICONERROR,
            );
        }
        InstallOutcome::WorkerFailed(code) => {
            let status = if code == EXIT_FAILED as u32 {
                "WinGet installation did not finish. You can try again.".into()
            } else {
                format!("The WinGet installer exited with code {code}. You can try again.")
            };
            set_text(window, GUIDE_STATUS, &status);
        }
        InstallOutcome::Complete(installation) => show_account_result(window, installation),
    }
}

fn show_account_result(window: HWND, installation: Installation) {
    let machine_status = match installation.provisioned {
        Provisioned::Installed => "App Installer was provisioned.",
        Provisioned::AlreadyCurrent => "App Installer was already up to date.",
    };
    let mut details = String::new();
    let account_status = match installation.version {
        Ok(version) => format!("WinGet {version} is ready for this user."),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            details = error.to_string();
            "WinGet verification was skipped.".into()
        }
        Err(error) => {
            details = format!(
                "{error}\r\n\r\nSign out and back in, then check the WinGet app execution alias in Windows Settings if WinGet is still unavailable."
            );
            "WinGet is unavailable for this user.".into()
        }
    };
    set_text(
        window,
        GUIDE_STATUS,
        &format!("{machine_status}\r\n{account_status}"),
    );
    if let Some(error) = installation.registration_error {
        details = if details.is_empty() {
            format!(
                "Could not request registration for this user. {error}\r\n\r\nThe existing WinGet alias responded to the version check."
            )
        } else {
            format!("Could not request registration for this user. {error}\r\n\r\n{details}")
        };
    }
    if !details.is_empty() {
        show_message(
            window,
            concat!(env!("CARGO_PKG_NAME"), " - WinGet for this Windows user"),
            &format!("{machine_status}\r\n{account_status}\r\n\r\n{details}"),
            MB_ICONWARNING,
        );
    }
}

unsafe extern "system" fn worker_dialog(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    // The callback forwards the original, synchronous paint-message arguments.
    if let Some(result) = unsafe { theme::paint(message, wparam, lparam, WORKER_DETAILS) } {
        return result;
    }
    // show_dialog supplies DialogJob<WorkerUpdate> and keeps it alive through every callback.
    let Some(job) =
        (unsafe { dialog_data::<DialogJob<WorkerUpdate>>(window, message, lparam).as_ref() })
    else {
        return 0;
    };
    match message {
        WM_INITDIALOG => {
            theme::refresh(window, WORKER_BUTTONS, Some(PROGRESS));
            // The marquee message uses numeric arguments and no borrowed data.
            unsafe { SendDlgItemMessageW(window, PROGRESS, PBM_SETMARQUEE, 1, 40) };
            if let Err(error) = job.start(window, |updates| {
                let report = deployment::provision(|progress| {
                    let _ = updates.send(WorkerUpdate::Progress(progress));
                });
                let _ = updates.send(WorkerUpdate::Finished(report));
            }) {
                worker_job_error(window, &error.to_string());
            }
        }
        WM_COMMAND if matches!(command_id(wparam), CLOSE | IDCANCEL | IDOK) => {
            close_worker(window, job)
        }
        WM_TIMER if wparam == JOB_TIMER => worker_updates(window, job),
        WM_CLOSE => close_worker(window, job),
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            theme::refresh(window, WORKER_BUTTONS, Some(PROGRESS));
            return 0;
        }
        _ => return 0,
    }
    1
}

fn close_worker(window: HWND, job: &DialogJob<WorkerUpdate>) {
    if !job.is_active() {
        end_dialog(window, EXIT_FAILED);
    }
}

fn worker_updates(window: HWND, job: &DialogJob<WorkerUpdate>) {
    loop {
        match job.next() {
            Some(Ok(WorkerUpdate::Progress(progress))) => show_provision_progress(window, progress),
            Some(Ok(WorkerUpdate::Finished(report))) => {
                job.finish(window);
                match report.result {
                    Ok(provisioned) => {
                        if let Some(warning) = report.cleanup_warning {
                            // Provisioning is complete, even if the cleanup warning keeps the worker open.
                            unsafe { SendDlgItemMessageW(window, PROGRESS, PBM_SETMARQUEE, 0, 0) };
                            set_text(
                                window,
                                WORKER_STATUS,
                                "App Installer provisioning is complete.",
                            );
                            show_message(
                                window,
                                concat!(env!("CARGO_PKG_NAME"), " - WinGet temporary files"),
                                &warning,
                                MB_ICONWARNING,
                            );
                        }
                        end_dialog(window, deployment::worker_exit(provisioned));
                    }
                    Err(error) => {
                        let mut details = error.to_string();
                        if let Some(warning) = report.cleanup_warning {
                            details.push_str("\r\n\r\n");
                            details.push_str(&warning);
                        }
                        worker_job_error(window, &details);
                    }
                }
                break;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                job.finish(window);
                worker_job_error(window, "The background operation ended unexpectedly.");
                break;
            }
            Some(Err(TryRecvError::Empty)) | None => break,
        }
    }
}

fn show_provision_progress(window: HWND, progress: ProvisionProgress) {
    let text = match progress {
        ProvisionProgress::CheckRelease => "Checking Microsoft's latest stable WinGet release…",
        ProvisionProgress::DownloadBundle(tag) => {
            set_text(
                window,
                WORKER_STATUS,
                &format!("Downloading and verifying App Installer ({tag})…"),
            );
            return;
        }
        ProvisionProgress::CheckProvisionedVersion => {
            "Checking the App Installer version provisioned on this PC…"
        }
        ProvisionProgress::DownloadDependencies => {
            "Downloading and verifying the dependencies and license…"
        }
        ProvisionProgress::ExtractDependencies => "Extracting the x86 and x64 dependencies…",
        ProvisionProgress::Deploy => {
            "Installing App Installer and its dependencies through Windows…"
        }
    };
    set_text(window, WORKER_STATUS, text);
}

fn worker_job_error(window: HWND, error: &str) {
    // The worker progress control accepts a numeric marquee flag.
    unsafe { SendDlgItemMessageW(window, PROGRESS, PBM_SETMARQUEE, 0, 0) };
    set_text(
        window,
        WORKER_STATUS,
        "WinGet installation could not finish.",
    );
    set_text(window, WORKER_DETAILS, error);
}

fn selection(window: HWND) -> Option<&'static Recommendation> {
    // LB_GETCURSEL has no input pointers and returns the row index or LB_ERR.
    let index = unsafe { SendDlgItemMessageW(window, SETTINGS_LIST, LB_GETCURSEL, 0, 0) };
    RECOMMENDATIONS.get(usize::try_from(index).ok()?)
}

fn show_selection(window: HWND) {
    if let Some(entry) = selection(window) {
        set_text(window, TITLE, entry.title);
        set_text(
            window,
            DETAILS,
            &format!("{}\r\n\r\n{}", entry.advice, entry.path),
        );
        let action = match entry.target {
            Target::DarkTheme => "&Apply dark theme",
            Target::Apply(_) => "&Apply",
            _ => "&Open settings",
        };
        set_text(window, SETTINGS_ACTION, action);
        let visibility = if entry.apply.is_some() {
            SW_SHOWNA
        } else {
            SW_HIDE
        };
        // The resource owns an optional Apply button that should never take focus when shown.
        unsafe { ShowWindow(GetDlgItem(window, SETTINGS_APPLY), visibility) };
    }
}

fn open_selection(window: HWND, apply: bool) {
    let Some(entry) = selection(window) else {
        return;
    };
    let target = if apply {
        let Some(preference) = entry.apply else {
            return;
        };
        Target::Apply(preference)
    } else {
        entry.target
    };
    let result = match target {
        Target::Settings(uri) => shell_open(window, OsStr::new(uri), ""),
        Target::SystemProgram(program, arguments) => system_directory().and_then(|directory| {
            shell_open(window, directory.join(program).as_os_str(), arguments)
        }),
        Target::DarkTheme => system_path(GetSystemWindowsDirectoryW).and_then(|directory| {
            shell_open(
                window,
                directory.join(r"Resources\Themes\dark.theme").as_os_str(),
                "",
            )
        }),
        Target::Apply(preference) => preferences::apply(window, entry.title, preference),
    };
    if let Err(error) = result {
        show_message(
            window,
            entry.title,
            &format!("{error}\r\n\r\n{}", entry.path),
            MB_ICONERROR,
        );
    }
}

fn shell_open(window: HWND, target: &OsStr, arguments: &str) -> io::Result<()> {
    let target = wide(target);
    let arguments = (!arguments.is_empty()).then(|| wide(arguments));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        hwnd: window,
        lpVerb: w!("open"),
        lpFile: target.as_ptr(),
        lpParameters: arguments.as_ref().map_or(null(), |args| args.as_ptr()),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    // The window is the active dialog and all referenced strings live through the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> io::Result<Self> {
        // This thread has no other COM setup. Each success is balanced in Drop on the same thread.
        let result = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
        if result < 0 {
            return Err(io::Error::other(format!(
                "Could not start Windows shell integration (0x{result:08x})."
            )));
        }
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // The guard is only used on its creating thread after successful initialization.
        unsafe { CoUninitialize() };
    }
}

fn system_directory() -> io::Result<PathBuf> {
    system_path(GetSystemDirectoryW)
}

fn system_path(read_path: unsafe extern "system" fn(*mut u16, u32) -> u32) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 260];
    loop {
        // Both supported APIs take a UTF-16 output buffer and report the required capacity.
        let length = unsafe { read_path(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        match length {
            0 => return Err(io::Error::last_os_error()),
            size if size < buffer.len() => return Ok(OsString::from_wide(&buffer[..size]).into()),
            size => buffer.resize(size + 1, 0),
        }
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

fn set_text(window: HWND, control: i32, text: &str) {
    let text = wide(text);
    // The active dialog owns this control and copies the terminated string during the call.
    unsafe { SetDlgItemTextW(window, control, text.as_ptr()) };
}

fn enable(window: HWND, control: i32, enabled: bool) {
    // These IDs identify controls in the active dialog's resource.
    unsafe { EnableWindow(GetDlgItem(window, control), enabled.into()) };
}

fn end_dialog(window: HWND, exit: i32) {
    // The window is a modal dialog created by DialogBoxParamW on this thread.
    unsafe { EndDialog(window, exit as isize) };
}

fn show_message(
    window: HWND,
    title: &str,
    text: &str,
    style: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
    let title = wide(title);
    let text = wide(text);
    // The owner is either the active dialog or NULL, and both strings live through the call.
    unsafe { MessageBoxW(window, text.as_ptr(), title.as_ptr(), style) }
}
