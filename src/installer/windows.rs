use super::Provisioned;
use crate::winget::{self, Asset, Result};
use base64::prelude::*;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

const LATEST_RELEASE: &str = "https://api.github.com/repos/microsoft/winget-cli/releases/latest";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const APP_INSTALLER_SCRIPT: &str = include_str!("../provision.ps1");

pub enum ProvisionProgress {
    CheckRelease,
    DownloadBundle(String),
    CheckProvisionedVersion,
    DownloadDependencies,
    ExtractDependencies,
    Deploy,
}

pub fn provision(
    system_directory: &Path,
    staging: &Path,
    mut progress: impl FnMut(ProvisionProgress),
) -> Result<Provisioned> {
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let config = ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(true)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(15 * 60)))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    progress(ProvisionProgress::CheckRelease);
    let mut response = agent
        .get(LATEST_RELEASE)
        .header(
            "User-Agent",
            concat!("postinstall/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("Could not reach the WinGet releases on GitHub. {error}"))?;
    let mut metadata = Vec::new();
    const MAX_METADATA: u64 = 1024 * 1024;
    response
        .body_mut()
        .as_reader()
        .take(MAX_METADATA + 1)
        .read_to_end(&mut metadata)?;
    if metadata.len() as u64 > MAX_METADATA {
        return Err("GitHub returned unexpectedly large release metadata.".into());
    }
    let release = winget::parse_release(&metadata)?;

    progress(ProvisionProgress::DownloadBundle(release.tag));
    let bundle = download(&agent, &release.bundle, staging)?;
    let version = winget::bundle_version(File::open(&bundle)?)?;

    progress(ProvisionProgress::CheckProvisionedVersion);
    let output = powershell(system_directory, "check")
        .env("POSTINSTALL_VERSION", &version)
        .output()?;
    let version_status = checked_output(output, "Windows PowerShell").map_err(|error| {
        format!("Could not check the provisioned App Installer version. {error}")
    })?;
    match version_status.trim() {
        "current" => return Ok(Provisioned::AlreadyCurrent),
        "needs-install" => {}
        _ => return Err("Windows did not report the provisioned App Installer version.".into()),
    }

    progress(ProvisionProgress::DownloadDependencies);
    let dependencies = download(&agent, &release.dependencies, staging)?;
    let license = download(&agent, &release.license, staging)?;

    progress(ProvisionProgress::ExtractDependencies);
    let packages = winget::extract_dependencies(File::open(dependencies)?, staging)?;
    let package_paths: Vec<_> = packages.iter().map(|path| path.as_os_str()).collect();

    progress(ProvisionProgress::Deploy);
    let output = powershell(system_directory, "install")
        .env("POSTINSTALL_BUNDLE", bundle)
        .env("POSTINSTALL_LICENSE", license)
        .env(
            "POSTINSTALL_DEPENDENCIES",
            package_paths.join(&OsString::from("|")),
        )
        .output()?;
    checked_output(output, "Windows PowerShell")
        .map_err(|error| format!("Windows could not provision App Installer. {error}"))?;
    Ok(Provisioned::Installed)
}

fn download(agent: &ureq::Agent, asset: &Asset, staging: &Path) -> Result<PathBuf> {
    let path = staging.join(&asset.name);
    let result = (|| -> Result<()> {
        let response = agent
            .get(&asset.url)
            .header(
                "User-Agent",
                concat!("postinstall/", env!("CARGO_PKG_VERSION")),
            )
            .call()?;
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        winget::copy_verified(asset, response.into_body().into_reader(), output)?;
        Ok(())
    })();
    result.map_err(|error| format!("Could not download or verify {}. {error}", asset.name))?;
    Ok(path)
}

pub fn register_current_user(system_directory: &Path) -> Result<()> {
    checked_output(
        powershell(system_directory, "register").output()?,
        "Windows PowerShell",
    )?;
    Ok(())
}

fn powershell(system_directory: &Path, stage: &str) -> Command {
    let home = system_directory.join("WindowsPowerShell/v1.0");
    let utf16_bytes: Vec<u8> = APP_INSTALLER_SCRIPT
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut command = Command::new(home.join("powershell.exe"));
    command
        .current_dir(system_directory)
        .env("PSModulePath", home.join("Modules"))
        .env("POSTINSTALL_STAGE", stage)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
        ])
        .arg(BASE64_STANDARD.encode(utf16_bytes))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null());
    command
}

fn checked_output(output: Output, program: &str) -> Result<String> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let details = String::from_utf8_lossy(&output.stderr);
    let details = details.trim();
    let message = if details.is_empty() {
        format!("{program} stopped with {}.", output.status)
    } else {
        details.to_owned()
    };
    Err(io::Error::other(message).into())
}
