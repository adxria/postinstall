# postinstall

postinstall is a portable utility for setting up Windows 11 IoT Enterprise LTSC 2024 on x64. Run `postinstall.exe` from the Windows account you are setting up. The 15 recommendations work offline and follow Windows’ app color mode. Mouse pointer options, keyboard repeat, the Sticky Keys shortcut, and access-key underlines each have **Apply** and **Open settings** buttons. **Apply** asks for confirmation and verifies the resulting setting.

## Install and verify WinGet

Choose **Install or update WinGet** with an internet connection and approve the administrator prompt. The worker checks Microsoft’s latest stable release, verifies its downloads, and provisions App Installer if needed. Temporary downloads go in a protected per-run folder under `%WINDIR%\Temp`. The worker removes it when it finishes. A forced shutdown can interrupt cleanup.

postinstall then requests registration for the account that launched it and checks the WinGet app execution alias. Open postinstall without administrator privileges to run this check. In a terminal opened by that account, run `winget --version` to verify the client and `winget search --source winget 7zip` to check the community source. If App Installer is provisioned and WinGet is unavailable, sign out and back in, then check the WinGet app execution alias in Windows Settings.

## Build

Use the Rust Windows target, `cargo-xwin`, and LLVM to build from Linux or WSL.

```sh
cargo xwin build --locked --release --target x86_64-pc-windows-msvc
```

Use Rust, MSVC Build Tools, and the Windows SDK to build on x64 Windows.

```powershell
cargo build --locked --release --target x86_64-pc-windows-msvc
```

Share `target/x86_64-pc-windows-msvc/release/postinstall.exe`. The EXE uses Windows components and needs no separately installed runtime.

Run `cargo test --locked` for installer outcome, download, and archive checks. Edit `src/settings.rs` to update the recommendations.
