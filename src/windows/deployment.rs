use super::{ComApartment, system_directory, system_path, wide, winget_probe};
use crate::installer::{self, Installation, ProvisionProgress, Provisioned};
use crate::winget::Result;
use std::ffi::c_void;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows_sys::w;

pub(super) const WORKER_ARG: &str = "--install-winget";
const EXIT_INSTALLED: i32 = 0;
pub(super) const EXIT_FAILED: i32 = 1;
const EXIT_CURRENT: i32 = 2;

pub(super) fn worker_exit(provisioned: Provisioned) -> i32 {
    match provisioned {
        Provisioned::Installed => EXIT_INSTALLED,
        Provisioned::AlreadyCurrent => EXIT_CURRENT,
    }
}

pub(super) enum InstallOutcome {
    Canceled,
    LaunchFailed(io::Error),
    WorkerFailed(u32),
    Complete(Installation),
}

pub(super) fn install_for_user(registering: impl FnOnce()) -> InstallOutcome {
    let provisioned = match run_elevated() {
        Ok(code) if code == EXIT_INSTALLED as u32 => Provisioned::Installed,
        Ok(code) if code == EXIT_CURRENT as u32 => Provisioned::AlreadyCurrent,
        Ok(code) => return InstallOutcome::WorkerFailed(code),
        Err(error) if error.raw_os_error() == Some(ERROR_CANCELLED as i32) => {
            return InstallOutcome::Canceled;
        }
        Err(error) => return InstallOutcome::LaunchFailed(error),
    };

    registering();
    InstallOutcome::Complete(installer::finish_installation(
        provisioned,
        || installer::register_current_user(&system_directory()?),
        winget_probe::version_after_registration,
    ))
}

pub(super) struct ProvisionReport {
    pub result: Result<Provisioned>,
    pub cleanup_warning: Option<String>,
}

pub(super) fn provision(progress: impl FnMut(ProvisionProgress)) -> ProvisionReport {
    let mut cleanup_warning = None;
    let result = (|| {
        let _exclusive = installation_lock()?;
        let system = system_directory()?;
        let stage = Staging::create()?;
        let result = installer::provision(&system, &stage.0, progress);
        cleanup_warning = stage.remove();
        result
    })();
    ProvisionReport {
        result,
        cleanup_warning,
    }
}

fn run_elevated() -> io::Result<u32> {
    let _com = ComApartment::initialize()?;
    let executable = wide(std::env::current_exe()?);
    let arguments = wide(WORKER_ARG);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: executable.as_ptr(),
        lpParameters: arguments.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    // All pointers refer to live terminated strings. NOCLOSEPROCESS transfers the process handle.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.hProcess.is_null() {
        return Err(io::Error::other(
            "Windows did not return the installer's process handle.",
        ));
    }
    // The shell returned a non-null, owned process handle. This worker thread can wait on it.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let mut code = 0;
    if unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0
        || unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(code)
}

pub(super) fn is_elevated() -> io::Result<bool> {
    let mut token = null_mut();
    // The process pseudo-handle is always available and the token slot is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // OpenProcessToken returned ownership of the token handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut size = 0;
    // TokenElevation writes a TOKEN_ELEVATION, whose storage and size we provide here.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn installation_lock() -> io::Result<OwnedHandle> {
    // Only the first process to create this kernel object can proceed. Closing it releases the lock.
    let mutex = unsafe { CreateMutexW(null(), 0, w!("Global\\postinstall.WinGet.Provision")) };
    let error = io::Error::last_os_error();
    if mutex.is_null() {
        return Err(error);
    }
    // The returned non-null handle is owned even if the mutex already existed.
    let mutex = unsafe { OwnedHandle::from_raw_handle(mutex) };
    if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
        return Err(io::Error::other(
            "Another window is already installing WinGet.",
        ));
    }
    Ok(mutex)
}

struct Staging(PathBuf);

impl Staging {
    fn create() -> Result<Self> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|error| format!("Could not create a temporary name. {error}"))?;
        let directory = system_path(GetSystemWindowsDirectoryW)?
            .join("Temp")
            .join(format!("postinstall-{:032x}", u128::from_le_bytes(random)));
        let path = wide(&directory);
        let mut descriptor = null_mut();

        // The descriptor permits SYSTEM and administrators and requires high integrity for writes.
        // The conversion allocates a descriptor and returns it through the writable slot.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)S:(ML;OICI;NW;;;HI)"),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // The unique path is created atomically with this descriptor, which the call only borrows.
        let created = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
        let error = io::Error::last_os_error();
        // LocalFree is the matching deallocator for a converted security descriptor.
        unsafe { LocalFree(descriptor.cast::<c_void>()) };
        if created == 0 {
            return Err(error.into());
        }
        Ok(Self(directory))
    }

    fn remove(mut self) -> Option<String> {
        let directory = std::mem::take(&mut self.0);
        match fs::remove_dir_all(&directory) {
            Ok(()) => None,
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => Some(format!(
                "Could not remove temporary files from {}. {error}\r\n\r\nYou can delete this folder as an administrator.",
                directory.display()
            )),
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
