use super::{show_message, wide};
use crate::settings::Preference;
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::{from_mut, null, null_mut, without_provenance_mut};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RegSetKeyValueW,
};
use windows_sys::Win32::UI::Accessibility::{SKF_HOTKEYACTIVE, SKF_STICKYKEYSON, STICKYKEYS};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const PERSIST_AND_NOTIFY: SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS =
    SPIF_UPDATEINIFILE | SPIF_SENDCHANGE;

pub(super) fn apply(window: HWND, title: &str, preference: Preference) -> io::Result<()> {
    let (prompt, change): (_, fn() -> io::Result<()>) = match preference {
        Preference::ClassicContextMenu => (
            "Use the classic right-click menu for this Windows account?",
            classic_context_menu,
        ),
        Preference::PrintScreen => (
            "Use Print Screen to copy the whole screen to the clipboard for this Windows account?",
            print_screen,
        ),
        Preference::MouseAcceleration => (
            "Turn off Enhance pointer precision for this Windows account?",
            disable_mouse_acceleration,
        ),
        Preference::KeyboardRepeat => (
            "Set the shortest keyboard repeat delay and fastest repeat rate?",
            set_keyboard_repeat,
        ),
        Preference::StickyKeysShortcut => (
            "Turn off the five-Shift shortcut for Sticky Keys?",
            disable_sticky_keys_shortcut,
        ),
        Preference::AccessKeyUnderlines => (
            "Always underline access keys in supported apps?",
            enable_access_key_underlines,
        ),
    };
    if show_message(
        window,
        title,
        prompt,
        MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
    ) != IDYES
    {
        return Ok(());
    }

    change()?;
    let message = match preference {
        Preference::ClassicContextMenu | Preference::PrintScreen => {
            "Applied. Sign out and back in for the change to take effect."
        }
        _ => "Applied and verified.",
    };
    show_message(window, title, message, MB_ICONINFORMATION);
    Ok(())
}

fn classic_context_menu() -> io::Result<()> {
    set_user_value(
        r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32",
        None,
        REG_SZ,
        &[0, 0],
    )
}

fn print_screen() -> io::Result<()> {
    set_user_value(
        r"Control Panel\Keyboard",
        Some("PrintScreenKeyForSnippingEnabled"),
        REG_DWORD,
        &0u32.to_le_bytes(),
    )
}

fn set_user_value(
    subkey: &str,
    name: Option<&str>,
    kind: REG_VALUE_TYPE,
    data: &[u8],
) -> io::Result<()> {
    let subkey = wide(subkey);
    let name = name.map(wide);
    // The strings are terminated and the registry reads only the supplied bytes.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ref().map_or(null(), |name| name.as_ptr()),
            kind,
            data.as_ptr().cast(),
            data.len() as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

fn mouse_parameters() -> io::Result<[i32; 3]> {
    let mut values = [0; 3];
    // SPI_GETMOUSE writes the two thresholds and acceleration into this array.
    unsafe { system_parameters(SPI_GETMOUSE, 0, values.as_mut_ptr().cast(), 0)? };
    Ok(values)
}

fn disable_mouse_acceleration() -> io::Result<()> {
    let mut values = mouse_parameters()?;
    // SPI_GETMOUSESPEED writes one integer. SPI_SETMOUSE reads the array and leaves speed alone.
    let speed = unsafe { read_uint(SPI_GETMOUSESPEED)? };
    values[2] = 0;
    unsafe {
        system_parameters(
            SPI_SETMOUSE,
            0,
            values.as_mut_ptr().cast(),
            PERSIST_AND_NOTIFY,
        )?;
        verify(mouse_parameters()? == values && read_uint(SPI_GETMOUSESPEED)? == speed)
    }
}

fn set_keyboard_repeat() -> io::Result<()> {
    const DELAY: u32 = 0;
    const SPEED: u32 = 31;

    // The keyboard getters take an integer output and the setters take a numeric uiParam.
    unsafe {
        let original_delay = read_uint(SPI_GETKEYBOARDDELAY)?;
        system_parameters(SPI_SETKEYBOARDDELAY, DELAY, null_mut(), PERSIST_AND_NOTIFY)?;
        if let Err(error) =
            system_parameters(SPI_SETKEYBOARDSPEED, SPEED, null_mut(), PERSIST_AND_NOTIFY)
        {
            let restore = system_parameters(
                SPI_SETKEYBOARDDELAY,
                original_delay,
                null_mut(),
                PERSIST_AND_NOTIFY,
            )
            .and_then(|()| verify(read_uint(SPI_GETKEYBOARDDELAY)? == original_delay));
            let message = match restore {
                Ok(()) => format!(
                    "Could not set the repeat rate. The original delay was restored. {error}"
                ),
                Err(restore) => format!(
                    "Could not set the repeat rate. {error}\r\n\r\nCould not restore the original delay. {restore}"
                ),
            };
            return Err(io::Error::other(message));
        }
        verify(
            read_uint(SPI_GETKEYBOARDDELAY)? == DELAY && read_uint(SPI_GETKEYBOARDSPEED)? == SPEED,
        )
    }
}

fn sticky_keys() -> io::Result<STICKYKEYS> {
    let mut keys = STICKYKEYS {
        cbSize: size_of::<STICKYKEYS>() as u32,
        ..Default::default()
    };
    // Windows writes into the sized STICKYKEYS structure.
    unsafe {
        system_parameters(
            SPI_GETSTICKYKEYS,
            keys.cbSize,
            from_mut(&mut keys).cast(),
            0,
        )?
    };
    Ok(keys)
}

fn disable_sticky_keys_shortcut() -> io::Result<()> {
    let mut keys = sticky_keys()?;
    keys.dwFlags &= !SKF_HOTKEYACTIVE;
    // Windows reads the size and updated flags from STICKYKEYS.
    unsafe {
        system_parameters(
            SPI_SETSTICKYKEYS,
            keys.cbSize,
            from_mut(&mut keys).cast(),
            PERSIST_AND_NOTIFY,
        )?;
    }
    let mask = SKF_HOTKEYACTIVE | SKF_STICKYKEYSON;
    verify(sticky_keys()?.dwFlags & mask == keys.dwFlags & mask)
}

fn enable_access_key_underlines() -> io::Result<()> {
    // SPI_SETKEYBOARDCUES takes TRUE as a value. SPI_GETKEYBOARDCUES writes a 32-bit BOOL.
    unsafe {
        system_parameters(
            SPI_SETKEYBOARDCUES,
            0,
            without_provenance_mut(1),
            PERSIST_AND_NOTIFY,
        )?;
        verify(read_uint(SPI_GETKEYBOARDCUES)? != 0)
    }
}

// The action must be a getter that writes a 32-bit integer or BOOL to pvParam.
unsafe fn read_uint(action: SYSTEM_PARAMETERS_INFO_ACTION) -> io::Result<u32> {
    let mut value = 0u32;
    unsafe { system_parameters(action, 0, from_mut(&mut value).cast(), 0)? };
    Ok(value)
}

// The caller supplies the value or live storage required by the specific SPI action.
unsafe fn system_parameters(
    action: SYSTEM_PARAMETERS_INFO_ACTION,
    ui_param: u32,
    data: *mut c_void,
    flags: SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
) -> io::Result<()> {
    if unsafe { SystemParametersInfoW(action, ui_param, data, flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn verify(matches: bool) -> io::Result<()> {
    if matches {
        Ok(())
    } else {
        Err(io::Error::other(
            "Windows did not report the requested setting.",
        ))
    }
}
