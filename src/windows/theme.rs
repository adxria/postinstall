use std::cell::Cell;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{COLORREF, ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::Controls::{
    CLR_DEFAULT, DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_NOACCEL, ODS_NOFOCUSRECT,
    ODS_SELECTED, ODT_BUTTON, PBM_SETBARCOLOR, PBM_SETBKCOLOR, SetWindowTheme,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::w;

const BACKGROUND: COLORREF = rgb(31, 31, 31);
const SURFACE: COLORREF = rgb(43, 43, 43);
const TEXT: COLORREF = rgb(245, 245, 245);
const ACCENT: COLORREF = rgb(140, 184, 255);

thread_local! {
    static DARK: Cell<Option<bool>> = const { Cell::new(None) };
    static BRUSHES: Option<(Brush, Brush)> = Some((Brush::new(BACKGROUND)?, Brush::new(SURFACE)?));
}

pub(super) fn refresh(window: HWND, buttons: &[(i32, i32)], progress: Option<i32>) {
    let dark = system_prefers_dark() && BRUSHES.with(Option::is_some);
    if DARK.replace(Some(dark)) == Some(dark) {
        return;
    }

    let enabled = i32::from(dark);
    // The dialog is live and DWM reads the BOOL during the attribute call.
    unsafe {
        DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&enabled as *const i32).cast(),
            size_of::<i32>() as u32,
        );
    }
    for &(id, default_style) in buttons {
        let style = if dark { BS_OWNERDRAW } else { default_style };
        // The ID and style refer to a button in this dialog's resource.
        unsafe { SendDlgItemMessageW(window, id, BM_SETSTYLE, style as usize, 1) };
    }
    if let Some(id) = progress {
        let theme = if dark { w!("") } else { null() };
        let background = if dark {
            SURFACE as isize
        } else {
            CLR_DEFAULT as isize
        };
        let bar = if dark {
            ACCENT as isize
        } else {
            CLR_DEFAULT as isize
        };
        // Progress bar color messages take effect with visual styles disabled on this control.
        unsafe {
            SetWindowTheme(GetDlgItem(window, id), theme, theme);
            SendDlgItemMessageW(window, id, PBM_SETBKCOLOR, 0, background);
            SendDlgItemMessageW(window, id, PBM_SETBARCOLOR, 0, bar);
        }
    }
    // Invalidate the dialog and its controls so they repaint using the new palette.
    unsafe {
        RedrawWindow(
            window,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
        )
    };
}

fn system_prefers_dark() -> bool {
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let mut light = 1u32;
    let mut size = size_of::<u32>() as u32;
    // Both APIs write into correctly sized output buffers. Contrast themes keep native colors.
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&mut contrast as *mut HIGHCONTRASTW).cast(),
            0,
        ) != 0
            && contrast.dwFlags & HCF_HIGHCONTRASTON == 0
            && RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&mut light as *mut u32).cast(),
                &mut size,
            ) == ERROR_SUCCESS
            && light == 0
    }
}

// The arguments must be the original values sent to this dialog's paint callback.
pub(super) unsafe fn paint(
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    details: i32,
) -> Option<isize> {
    if DARK.get() != Some(true) {
        return None;
    }
    if message == WM_DRAWITEM {
        // Windows owns the DRAWITEMSTRUCT for this synchronous notification.
        let item = unsafe { (lparam as *const DRAWITEMSTRUCT).as_ref() }?;
        return (item.CtlType == ODT_BUTTON && draw_button(item)).then_some(1);
    }

    let is_field = match message {
        WM_CTLCOLORLISTBOX => true,
        WM_CTLCOLORDLG => false,
        // Read-only edits send WM_CTLCOLORSTATIC, with the control handle in LPARAM.
        WM_CTLCOLORSTATIC => (unsafe { GetDlgCtrlID(lparam as HWND) }) == details,
        _ => return None,
    };
    let context = wparam as HDC;
    // The message supplied a live device context. The returned brush outlives the paint call.
    unsafe {
        SetTextColor(context, TEXT);
        SetBkColor(context, if is_field { SURFACE } else { BACKGROUND });
    }
    BRUSHES.with(|brushes| {
        let (background, surface) = brushes.as_ref()?;
        Some(if is_field { surface.0 } else { background.0 } as isize)
    })
}

fn draw_button(item: &DRAWITEMSTRUCT) -> bool {
    let context = item.hDC;
    let mut label = [0; 128];
    let pressed = item.itemState & ODS_SELECTED != 0;
    let disabled = item.itemState & ODS_DISABLED != 0;
    let focused = item.itemState & (ODS_FOCUS | ODS_NOFOCUSRECT) == ODS_FOCUS;
    let mut bounds = item.rcItem;

    // Windows supplies a live DC and a button window for this draw notification.
    unsafe {
        let state = SaveDC(context);
        if state == 0 {
            return false;
        }
        let brush = GetStockObject(DC_BRUSH).cast();
        let background = if disabled {
            SURFACE
        } else if pressed {
            rgb(67, 67, 67)
        } else {
            rgb(55, 55, 55)
        };
        SetDCBrushColor(context, background);
        FillRect(context, &bounds, brush);
        SetDCBrushColor(context, if focused { ACCENT } else { rgb(103, 103, 103) });
        FrameRect(context, &bounds, brush);
        SetBkMode(context, TRANSPARENT as i32);
        SetTextColor(context, if disabled { rgb(161, 161, 161) } else { TEXT });
        let font = SendMessageW(item.hwndItem, WM_GETFONT, 0, 0);
        if font != 0 {
            SelectObject(context, font as HGDIOBJ);
        }
        let length = GetWindowTextW(item.hwndItem, label.as_mut_ptr(), label.len() as i32);
        let mut format = DT_SINGLELINE | DT_VCENTER | DT_CENTER;
        if item.itemState & ODS_NOACCEL != 0 {
            format |= DT_HIDEPREFIX;
        }
        DrawTextW(context, label.as_ptr(), length, &mut bounds, format);
        RestoreDC(context, state);
    }
    true
}

struct Brush(HBRUSH);

impl Brush {
    fn new(color: COLORREF) -> Option<Self> {
        // CreateSolidBrush returns an owned handle or NULL on failure.
        let handle = unsafe { CreateSolidBrush(color) };
        (!handle.is_null()).then_some(Self(handle))
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        // The dialog has finished painting before its thread-local brushes are dropped.
        unsafe { DeleteObject(self.0) };
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    u32::from_le_bytes([red, green, blue, 0])
}
